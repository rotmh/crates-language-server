use taplo::{
    dom::{
        Node,
        node::{DomNode, Key},
    },
    rowan::TextRange,
};
use tower_lsp::lsp_types::{self, Position, Range};

pub const DEPENDENCIES_KEYS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse toml document")]
    Parse,
}

#[derive(Debug)]
pub struct Dependency {
    pub name: Span<String>,
    pub version: Option<Span<Option<semver::Version>>>,
    pub features: Option<Vec<Span<String>>>,
}

impl Dependency {
    pub fn parse(s: &str, key: &Key, node: &Node) -> Result<Self, Error> {
        let name = Self::parse_name(key, s).ok_or(Error::Parse)?;
        let version = Self::parse_version(node, s);
        let features = Self::parse_features(node, s);

        Ok(Self {
            name,
            version,
            features,
        })
    }
}

impl Dependency {
    const VERSION_KEY: &str = "version";
    const FEATURES_KEY: &str = "features";

    fn parse_version(node: &Node, s: &str) -> Option<Span<Option<semver::Version>>> {
        let value = match node {
            Node::Str(s) => Some(s.clone()),
            Node::Table(t) if let Some(Node::Str(s)) = t.get(Self::VERSION_KEY) => Some(s),
            _ => None,
        };
        value.and_then(|value| {
            let range = text_range_to_range(value.syntax()?.text_range());
            let range = range_to_positions(s, range);
            let value = semver::Version::parse(value.value()).ok();
            Some(Span::new(value, range))
        })
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
            if c == '\n' && i < idx {
                (line + 1, i)
            } else {
                (line, line_pos)
            }
        })
}

pub fn idx_to_position(s: &str, idx: usize) -> lsp_types::Position {
    let (line, line_idx) = line_of_idx(s, idx);
    lsp_types::Position {
        line: line as u32,
        character: if line == 0 {
            idx - line_idx
        } else {
            idx - (line_idx + 1)
        } as u32,
    }
}

pub fn range_to_positions(s: &str, r: std::ops::Range<usize>) -> lsp_types::Range {
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
        assert_eq!(range_to_positions(s, 0..2), lsp_types::Range {
            start: lsp_types::Position::new(0, 0),
            end: lsp_types::Position::new(0, 2),
        });
        // multiline
        assert_eq!(range_to_positions(s, 6..10), lsp_types::Range {
            start: lsp_types::Position::new(0, 6),
            end: lsp_types::Position::new(1, 1),
        });
        // to line end
        assert_eq!(range_to_positions(s, 13..14), lsp_types::Range {
            start: lsp_types::Position::new(2, 0),
            end: lsp_types::Position::new(2, 1),
        });
    }
}
