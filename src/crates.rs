use std::collections::BTreeMap;

use serde::Deserialize;

const REGISTRY_URL: &str = "https://index.crates.io";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to fetch `{url}`")]
    Request { url: String },
    #[error("failed to parse body of the index of crate `{name}`")]
    Parse { name: String },
}

#[derive(Debug)]
pub struct Index {
    pub name: String,
    pub entries: Vec<Entry>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Index {
    pub fn parse(name: String, json_entries: &str) -> Result<Self> {
        // OPTIMIZE: is counting the lines here worth it?
        let mut entries = Vec::with_capacity(json_entries.lines().count());

        for line in json_entries.lines() {
            let entry =
                serde_json::from_str(line).map_err(|_| Error::Parse { name: name.clone() })?;
            entries.push(entry);
        }

        Ok(Self { name, entries })
    }

    pub fn latest(&self) -> &Entry {
        // FIXME: don't use all thses `.unwrap()`s
        self.entries
            .iter()
            .max_by(|a, b| {
                let version_a = semver::Version::parse(&a.vers).unwrap();
                let version_b = semver::Version::parse(&b.vers).unwrap();
                version_a.cmp(&version_b)
            })
            .unwrap()
    }
}

/// https://doc.rust-lang.org/cargo/reference/registry-index.html#json-schema
#[derive(Deserialize, Debug)]
pub struct Entry {
    /// The name of the package.
    /// This must only contain alphanumeric, `-`, or `_` characters.
    pub name: String,
    /// The version of the package this row is describing.
    /// This must be a valid version number according to the Semantic
    /// Versioning 2.0.0 spec at https://semver.org/.
    pub vers: String,
    /// Array of direct dependencies of the package.
    pub deps: Vec<Dependency>,
    /// A SHA256 checksum of the `.crate` file.
    pub cksum: String,
    /// Set of features defined for the package.
    /// Each feature maps to an array of features or dependencies it enables.
    /// May be omitted since Cargo 1.84.
    pub features: Option<BTreeMap<String, Vec<String>>>,
    /// Boolean of whether or not this version has been yanked.
    pub yanked: bool,
    /// The `links` string value from the package's manifest, or null if not
    /// specified. This field is optional and defaults to null.
    pub links: Option<String>,
    /// An unsigned 32-bit integer value indicating the schema version of this
    /// entry.
    ///
    /// If this is not specified, it should be interpreted as the default of 1.
    ///
    /// Cargo (starting with version 1.51) will ignore versions it does not
    /// recognize. This provides a method to safely introduce changes to index
    /// entries and allow older versions of cargo to ignore newer entries it
    /// doesn't understand. Versions older than 1.51 ignore this field, and
    /// thus may misinterpret the meaning of the index entry.
    ///
    /// The current values are:
    ///
    /// * 1: The schema as documented here, not including newer additions.
    ///      This is honored in Rust version 1.51 and newer.
    /// * 2: The addition of the `features2` field.
    ///      This is honored in Rust version 1.60 and newer.
    #[serde(default = "return_1")]
    pub v: u32,
    /// This optional field contains features with new, extended syntax.
    /// Specifically, namespaced features (`dep:`) and weak dependencies
    /// (`pkg?/feat`).
    ///
    /// This is separated from `features` because versions older than 1.19
    /// will fail to load due to not being able to parse the new syntax, even
    /// with a `Cargo.lock` file.
    ///
    /// Cargo will merge any values listed here with the "features" field.
    ///
    /// If this field is included, the "v" field should be set to at least 2.
    ///
    /// Registries are not required to use this field for extended feature
    /// syntax, they are allowed to include those in the "features" field.
    /// Using this is only necessary if the registry wants to support cargo
    /// versions older than 1.19, which in practice is only crates.io since
    /// those older versions do not support other registries.
    pub features2: Option<BTreeMap<String, Vec<String>>>,
    /// The minimal supported Rust version (optional)
    /// This must be a valid version requirement without an operator (e.g. no `=`)
    pub rust_version: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Dependency {
    /// Name of the dependency.
    /// If the dependency is renamed from the original package name,
    /// this is the new name. The original package name is stored in
    /// the `package` field.
    pub name: String,
    /// The SemVer requirement for this dependency.
    /// This must be a valid version requirement defined at
    /// https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html.
    pub req: String,
    /// Array of features (as strings) enabled for this dependency.
    /// May be omitted since Cargo 1.84.
    pub features: Option<Vec<String>>,
    /// Boolean of whether or not this is an optional dependency.
    /// Since Cargo 1.84, defaults to `false` if not specified.
    #[serde(default = "return_false")]
    pub optional: bool,
    /// Boolean of whether or not default features are enabled.
    /// Since Cargo 1.84, defaults to `true` if not specified.
    #[serde(default = "return_true")]
    pub default_features: bool,
    /// The target platform for the dependency.
    /// If not specified or `null`, it is not a target dependency.
    /// Otherwise, a string such as "cfg(windows)".
    pub target: Option<String>,
    /// The dependency kind.
    /// "dev", "build", or "normal".
    /// If not specified or `null`, it defaults to "normal".
    #[serde(default)]
    pub kind: DependencyKind,
    /// The URL of the index of the registry where this dependency is
    /// from as a string. If not specified or `null`, it is assumed the
    /// dependency is in the current registry.
    pub registry: Option<String>,
    /// If the dependency is renamed, this is a string of the actual
    /// package name. If not specified or `null`, this dependency is not
    /// renamed.
    pub package: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    Dev,
    Build,
    Normal,
}

impl Default for DependencyKind {
    fn default() -> Self {
        Self::Normal
    }
}

fn return_true() -> bool {
    true
}

fn return_false() -> bool {
    false
}

fn return_1() -> u32 {
    1
}

pub async fn fetch(name: &str) -> Result<Index> {
    let path = index_path(name);
    let url = format!("{REGISTRY_URL}/{path}");

    // TODO: use a client instance.
    let body = reqwest::get(&url)
        .await
        .map_err(|_| Error::Request { url: url.clone() })?
        .text()
        .await
        .map_err(|_| Error::Parse {
            name: name.to_owned(),
        })?;

    Index::parse(name.to_owned(), &body)
}

/// Get the path to the index file of the crate according to [Cargo's docs].
///
/// # Panics
///
/// The function will panic for empty names.
///
/// [Cargo's docs]: https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
fn index_path(name: &str) -> String {
    // the lint is about comparing to zero, but here we check if it's larger
    // than zero, which is more idiomatic in this case than `.is_empty()`.
    #[allow(clippy::len_zero)]
    {
        assert!(name.len() > 0);
    }

    match name.len() {
        1 => format!("1/{}", name),
        2 => format!("2/{}", name),
        3 => {
            // the lint is about iterators, but when getting a character in a
            // specific index, using `.nth()` is more readable.
            #[allow(clippy::iter_nth_zero)]
            let first_character = name.chars().nth(0).unwrap();
            format!("3/{}/{}", first_character, name)
        }
        _ => {
            let first_two: &str = &name[0..2];
            let second_two: &str = &name[2..4];
            format!("{}/{}/{}", first_two, second_two, name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_path() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("abcd"), "ab/cd/abcd");
        assert_eq!(index_path("cargo"), "ca/rg/cargo");
    }

    #[tokio::test]
    async fn test_working_fetch() {
        fetch("base64").await.unwrap();
    }

    #[tokio::test]
    async fn test_failing_fetch() {
        fetch("my_name_is_inigo_montoya_and_there_is_no_way_there_is_a_crate_with_this_name")
            .await
            .unwrap_err();
    }
}
