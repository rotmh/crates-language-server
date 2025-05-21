use std::path::{Path, PathBuf};

use taplo::{
    dom::{
        self, Node,
        node::{DomNode, Key},
    },
    rowan::TextRange,
};
use tower_lsp::lsp_types::{self, Position, Range};

pub const DEPENDENCIES_KEYS: &[&str] =
    &["dependencies", "dev-dependencies", "build-dependencies"];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse toml document")]
    Parse,
}

#[derive(Debug)]
pub enum Kind {
    /// Pull the dependency from crates.io.
    Registry,
    Git(GitKind),
    Local(LocalKind),
}

#[derive(Debug)]
pub struct LocalKind {
    path: Span<PathBuf>,
}

#[derive(Debug)]
pub struct GitKind {
    url: Span<String>,
    specifier: Option<GitSpecifier>,
}

#[derive(Debug)]
pub enum GitSpecifier {
    Branch(Span<String>),
    Tag(Span<String>),
    Rev(Span<String>),
}

#[derive(Debug)]
pub struct Dependency {
    pub kind: Kind,
    pub name: Span<String>,
    pub version: Option<Span<Option<semver::VersionReq>>>,
    pub features: Option<Vec<Span<String>>>,
}

impl Dependency {
    pub fn parse(s: &str, key: &Key, node: &Node) -> Result<Self, Error> {
        let name = Self::parse_name(key, s).ok_or(Error::Parse)?;
        let version = Self::parse_version(node, s);
        let features = Self::parse_features(node, s);

        let kind = Self::parse_local(node, s)
            .map(Kind::Local)
            .or_else(|| Self::parse_git(node, s).map(Kind::Git))
            .unwrap_or(Kind::Registry);

        Ok(Self { name, kind, version, features })
    }
}

impl Dependency {
    const VERSION_KEY: &str = "version";
    const FEATURES_KEY: &str = "features";
    const PATH_KEY: &str = "path";
    const REV_KEY: &str = "rev";
    const TAG_KEY: &str = "tag";
    const BRANCH_KEY: &str = "branch";
    const GIT_KEY: &str = "git";

    fn parse_git(node: &Node, s: &str) -> Option<GitKind> {
        let table = node.as_table()?;

        let url = Span::parse(
            table.get(Self::GIT_KEY)?.as_str()?,
            |s| Some(s.to_owned()),
            s,
        )?;

        let parse_specifier = |key, varient| {
            let span = table.get(key)?;
            Span::parse(span.as_str()?, |s| Some(s.to_owned()), s).map(varient)
        };

        let specifier = [
            (Self::REV_KEY, GitSpecifier::Rev as fn(_) -> GitSpecifier),
            (Self::BRANCH_KEY, GitSpecifier::Branch),
            (Self::TAG_KEY, GitSpecifier::Tag),
        ]
        .iter()
        .find_map(|(k, v)| parse_specifier(*k, v));

        Some(GitKind { url, specifier })
    }

    fn parse_local(node: &Node, s: &str) -> Option<LocalKind> {
        let table = node.as_table()?.get(Self::PATH_KEY)?;
        let path = Span::parse(
            table.as_str()?,
            |s| Some(Path::new(s).to_path_buf()),
            s,
        )?;
        Some(LocalKind { path })
    }

    fn parse_version(
        node: &Node,
        s: &str,
    ) -> Option<Span<Option<semver::VersionReq>>> {
        let value = node.as_str().cloned().or_else(|| {
            node.as_table()?.get(Self::VERSION_KEY)?.try_into_str().ok()
        })?;
        let range = text_range_to_range(value.syntax()?.text_range());
        let range = range_to_positions(s, range);
        let value = semver::VersionReq::parse(value.value()).ok();
        Some(Span::new(value, range))
    }

    fn parse_features(node: &Node, s: &str) -> Option<Vec<Span<String>>> {
        let features = node
            .as_table()?
            .get(Self::FEATURES_KEY)?
            .as_array()?
            .items()
            .read()
            .iter()
            .filter_map(|elem| {
                let value = elem.as_str()?.value().to_owned();
                let range = text_range_to_range(elem.syntax()?.text_range());
                let range = range_to_positions(s, range);
                Some(Span::new(value, range))
            })
            .collect();

        Some(features)
    }

    fn parse_name(key: &Key, s: &str) -> Option<Span<String>> {
        let value = key.to_string();
        let range = text_range_to_range(key.text_ranges().nth(0)?);
        let range = range_to_positions(s, range);
        Some(Span::new(value, range))
    }
}

pub fn text_range_to_range(text_range: TextRange) -> std::ops::Range<usize> {
    usize::from(text_range.start())..usize::from(text_range.end())
}

#[derive(Debug)]
pub struct Span<T> {
    pub value: T,
    pub range: Range,
}

impl<T> Span<T> {
    pub fn new(value: T, range: Range) -> Self {
        Self { value, range }
    }

    fn parse<F>(string: &dom::node::Str, f: F, s: &str) -> Option<Span<T>>
    where
        F: Fn(&str) -> Option<T>,
    {
        let value = f(string.value())?;
        let range = text_range_to_range(string.syntax()?.text_range());
        let range = range_to_positions(s, range);
        Some(Span::new(value, range))
    }

    pub fn contains_pos(&self, pos: Position) -> bool {
        let (start, end) = (self.range.start, self.range.end);
        !(!(start.line..=end.line).contains(&pos.line)
            || (start.line == pos.line && pos.character < start.character)
            || (end.line == pos.line && pos.character > end.character))
    }
}

fn line_of_idx(s: &str, idx: usize) -> (usize, usize) {
    s.chars()
        .enumerate()
        .fold((0, 0), |(line, line_pos), (i, c)| {
            // FIXME: stuff with '\r'
            if c == '\n' && i < idx { (line + 1, i) } else { (line, line_pos) }
        })
}

pub fn idx_to_position(s: &str, idx: usize) -> lsp_types::Position {
    let (line, line_idx) = line_of_idx(s, idx);
    lsp_types::Position {
        line: line as u32,
        character: if line == 0 { idx - line_idx } else { idx - (line_idx + 1) }
            as u32,
    }
}

pub fn range_to_positions(
    s: &str,
    r: std::ops::Range<usize>,
) -> lsp_types::Range {
    lsp_types::Range {
        start: idx_to_position(s, r.start),
        end: idx_to_position(s, r.end),
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_range_to_positions() {
        let s = indoc! {r#"
            12345678
            480
            3
        "#};

        // NOTE: the positions are zero-indexed and the end is exclusive

        // basic
        assert_eq!(
            range_to_positions(s, 0..2),
            lsp_types::Range {
                start: lsp_types::Position::new(0, 0),
                end: lsp_types::Position::new(0, 2),
            }
        );
        // multiline
        assert_eq!(
            range_to_positions(s, 6..10),
            lsp_types::Range {
                start: lsp_types::Position::new(0, 6),
                end: lsp_types::Position::new(1, 1),
            }
        );
        // to line end
        assert_eq!(
            range_to_positions(s, 13..14),
            lsp_types::Range {
                start: lsp_types::Position::new(2, 0),
                end: lsp_types::Position::new(2, 1),
            }
        );
    }
}
