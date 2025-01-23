use std::{collections::BTreeMap, str::FromStr};

use cargo_util_schemas::manifest::{InheritableDependency, PackageName};
use serde::Deserialize;
use toml_edit::{ImDocument, Item};
use tower_lsp::lsp_types;

const DEPENDENCIES_KEY: &str = "dependencies";

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
                .filter_map(|d| {
                    deps.get_key_value(d.0.as_str())
                        .and_then(|(k, _)| k.span())
                        .map(|r| (d.0, (range_to_positions(s, r), d.1)))
                })
                .collect(),
        })
    }
}

fn line_of_idx(s: &str, idx: usize) -> (usize, usize) {
    s.chars()
        .enumerate()
        .fold((0, 0), |(line, line_pos), (i, c)| {
            // FIXME: stuff with \r
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
    use cargo_util_schemas::manifest::TomlDependency;

    use super::*;

    #[test]
    fn parse_dependencies() {
        let s = r#"[dependencies]
serde = "1""#;

        let deps = Dependencies::from_str(s).unwrap();
        let serde = deps.crates.get("serde").unwrap();

        assert_eq!(serde.0, lsp_types::Range {
            start: lsp_types::Position::new(1, 0),
            end: lsp_types::Position::new(1, 5),
        });
        assert!(matches!(
            &serde.1,
            InheritableDependency::Value(TomlDependency::Simple(version)) if version == "1"
        ));
    }

    #[test]
    fn test_range_to_positions() {
        let s = r"12345678
480
3
        ";

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
