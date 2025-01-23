use std::{collections::BTreeMap, str::FromStr};

use cargo_util_schemas::manifest::{InheritableDependency, PackageName};
use serde::Deserialize;
use toml_edit::{ImDocument, Item, Value};
use tower_lsp::lsp_types::{self, Position};

pub const DEPENDENCIES_KEY: &str = "dependencies";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse toml document")]
    Parse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CargoManifest {
    dependencies: Option<BTreeMap<PackageName, InheritableDependency>>,
}

pub struct Dependencies {
    pub crates: BTreeMap<PackageName, (lsp_types::Range, InheritableDependency)>,
}

impl FromStr for Dependencies {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let doc = ImDocument::parse(s).map_err(|_| Error::Parse)?;
        let Some(deps) = doc.get(DEPENDENCIES_KEY).and_then(Item::as_table) else {
            return Ok(Self {
                crates: BTreeMap::new(),
            });
        };

        let manifest: CargoManifest = toml::from_str(s).map_err(|_| Error::Parse)?;
        let Some(dependencies) = manifest.dependencies else {
            return Ok(Self {
                crates: BTreeMap::new(),
            });
        };

        Ok(Self {
            crates: dependencies
                .into_iter()
                .filter_map(|(name, dep)| {
                    deps.get_key_value(name.as_str())
                        .and_then(|(key, _)| key.span())
                        .map(|rng| (name, (range_to_positions(s, rng), dep)))
                })
                .collect(),
        })
    }
}

/// If the pos is inside a dependecy's name, returns the the name.
/// Otherwise, returns [`None`].
///
/// # Examples
///
/// ```
/// # use crates_language_server::parse::pos_in_dependency_name;
/// use tower_lsp::lsp_types::Position;
///
/// let s = r#"
/// [dependencies]
/// serde = "1"
/// "#;
/// let pos = Position::new(2, 2);
///
/// assert_eq!(pos_in_dependency_name(s, pos), Some("serde".to_owned()));
/// ```
pub fn pos_in_dependency_name(s: &str, pos: Position) -> Option<String> {
    let doc = ImDocument::from_str(s).ok()?;

    doc.get(DEPENDENCIES_KEY)?
        .as_table()?
        .get_values()
        .iter()
        .filter(|&(keys, _)| (keys.len() == 1))
        .map(|(keys, _)| keys.first().unwrap())
        .filter_map(|name| name.span().map(|rng| (name.to_string(), rng)))
        .find(|(_, rng)| is_pos_in_range(s, rng.to_owned(), pos))
        .map(|(name, _)| name)
}

/// If the pos is inside the dependecy's version string, returns the
/// dependency name. Otherwise, returns [`None`].
pub fn pos_in_dependency_version(s: &str, pos: Position) -> Option<String> {
    let doc = ImDocument::from_str(s).ok()?;

    doc.get(DEPENDENCIES_KEY)?
        .as_table()?
        .iter()
        .find(|(_, value)| pos_in_version_field(s, pos, value))
        .map(|(name, _)| name.to_owned())
}

fn pos_in_version_field(s: &str, pos: Position, value: &Item) -> bool {
    if let Item::Value(value) = value {
        let version_rng = match value {
            Value::String(version) if let Some(rng) = version.span() => Some(rng),
            Value::InlineTable(table)
                if let Some(Value::String(version)) = table.get("version")
                    && let Some(rng) = version.span() =>
            {
                Some(rng)
            }
            _ => None,
        };
        version_rng.is_some_and(|rng| is_pos_in_range(s, rng.to_owned(), pos))
    } else {
        false
    }
}

fn is_pos_in_range(s: &str, rng: std::ops::Range<usize>, pos: Position) -> bool {
    eprintln!("trying to determine whether the pos: {pos:#?} in the range {rng:#?}");

    let positions = range_to_positions(s, rng);
    let (start, end) = (positions.start, positions.end);
    !(!(start.line..=end.line).contains(&pos.line)
        || (start.line == pos.line && pos.character < start.character)
        || (end.line == pos.line && pos.character > end.character))
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

fn idx_to_position(s: &str, idx: usize) -> lsp_types::Position {
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

fn range_to_positions(s: &str, r: std::ops::Range<usize>) -> lsp_types::Range {
    lsp_types::Range {
        start: idx_to_position(s, r.start),
        end: idx_to_position(s, r.end),
    }
}

#[cfg(test)]
mod tests {
    use cargo_util_schemas::manifest::{TomlDependency, TomlDetailedDependency};
    use indoc::indoc;

    use super::*;

    #[test]
    fn parse_dependencies() {
        let s = indoc! {r#"
            [dependencies]
            serde = { version = "1" }
        "#};

        let deps = Dependencies::from_str(s).unwrap();
        let serde = deps.crates.get("serde").unwrap();

        assert_eq!(serde.0, lsp_types::Range {
            start: lsp_types::Position::new(1, 0),
            end: lsp_types::Position::new(1, 5),
        });
        assert!(matches!(
            &serde.1,
            InheritableDependency::Value(TomlDependency::Detailed(
                TomlDetailedDependency { version: Some(version), .. }
            )) if version == "1"
        ));
    }

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
