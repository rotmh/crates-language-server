use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use reqwest::Response;
use serde::Deserialize;
use tokio::sync::Mutex;

const REGISTRY_URL: &str = "https://index.crates.io";
const API_URL: &str = "https://crates.io/api/v1/crates";

pub const DOCS_RS_URL: &str = "https://docs.rs";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to fetch `{url}`")]
    Request { url: String },
    #[error("failed to parse body of the index of crate `{name}`")]
    Parse { name: String },
}

/// A cache for a "latest" entry for crates.
#[derive(Debug)]
pub struct RegistryCache {
    crates: Arc<Mutex<HashMap<String, Latest>>>,
    client: reqwest::Client,
    last_api_request: Arc<Mutex<Instant>>,
}

impl RegistryCache {
    pub fn new() -> Self {
        Self {
            crates: Arc::new(Mutex::new(HashMap::new())),
            client: reqwest::ClientBuilder::new()
                .user_agent("crates-language-server (github.com/rotmh)")
                .build()
                .unwrap_or_default(),
            last_api_request: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Fetch description only if 1 minute passed since last API request.
    ///
    /// This rate limiting is required because it's one of [`crates.io`'s limits]:
    ///
    /// * "A maximum of 1 request per second"
    ///
    /// [`crates.io`'s limits]: https://crates.io/data-access#api
    async fn fetch_description_rated(&self, name: &str) -> Option<String> {
        let last_req = *self.last_api_request.lock().await;
        let since_last_req = Instant::now().duration_since(last_req);

        if since_last_req > Duration::from_secs(1) {
            *self.last_api_request.lock().await = Instant::now();
            self.fetch_description(name).await.ok()
        } else {
            None
        }
    }

    async fn fetch_description(&self, name: &str) -> Result<String> {
        #[derive(Debug, Deserialize)]
        struct ApiResponse {
            #[serde(rename = "crate")]
            krate: Krate,
        }
        #[derive(Debug, Deserialize)]
        struct Krate {
            description: String,
        }

        self.fetch_content(&api_url(name))
            .await
            .and_then(|body| {
                serde_json::from_str(&body).map_err(|_| Error::Parse {
                    name: name.to_owned(),
                })
            })
            .map(|res: ApiResponse| res.krate.description)
    }

    async fn fetch_endpoint(&self, url: &str) -> Result<Response> {
        let res = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| Error::Request {
                url: url.to_owned(),
            })?;

        res.status()
            .is_success()
            .then_some(res)
            .ok_or(Error::Request {
                url: url.to_owned(),
            })
    }

    async fn fetch_content(&self, url: &str) -> Result<String> {
        let res: Response = self.fetch_endpoint(url).await?;
        res.text().await.map_err(|_| Error::Request {
            url: url.to_owned(),
        })
    }

    /// Checks if a crate is available.
    ///
    /// This function is meant for checking whether a crate name is a valid
    /// name of an existing crate.
    pub async fn is_availabe(&self, name: &str) -> bool {
        // we check the cache first, and then (if entry does not exist) we
        // check the crates.io endpoint.
        self.crates.lock().await.contains_key(name)
            || self.fetch_endpoint(&index_url(name)).await.is_ok()
    }

    pub async fn fetch(&self, name: &str) -> Result<Latest> {
        if let Some(entry) = self.crates.lock().await.get_mut(name) {
            let description = if let Some(description) = &entry.description {
                Some(description.to_owned())
            } else {
                let description = self.fetch_description_rated(name).await;
                if let Some(description) = &description {
                    entry.description = Some(description.to_owned());
                }
                description
            };
            return Ok(Latest {
                version: entry.version.clone(),
                features: entry.features.clone(),
                description,
            });
        }
        let entries = self
            .fetch_content(&index_url(name))
            .await
            .and_then(|body| Index::parse(name, &body))?
            .entries;
        let latest = entries.last().ok_or_else(|| Error::Parse {
            name: name.to_owned(),
        })?;

        let version = semver::Version::parse(&latest.vers).map_err(|_| Error::Parse {
            name: name.to_owned(),
        })?;
        let features = if latest.v == 2 {
            latest.features2.clone()
        } else {
            latest.features.clone()
        };

        let latest = Latest {
            description: None,
            version,
            features,
        };

        self.crates
            .lock()
            .await
            .insert(name.to_owned(), latest.clone());

        Ok(latest)
    }
}

impl Default for RegistryCache {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: better name
#[derive(Clone, Debug)]
pub struct Latest {
    pub version: semver::Version,
    pub features: Option<HashMap<String, Vec<String>>>,
    pub description: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    pub fn parse(name: &str, json_entries: &str) -> Result<Self> {
        // OPTIMIZE: is counting the lines here worth it?
        let mut entries = Vec::with_capacity(json_entries.lines().count());

        for line in json_entries.lines() {
            let entry = serde_json::from_str(line).map_err(|_| Error::Parse {
                name: name.to_owned(),
            })?;
            entries.push(entry);
        }

        Ok(Self { entries })
    }
}

/// https://doc.rust-lang.org/cargo/reference/registry-index.html#json-schema
#[derive(Deserialize, Debug)]
struct Entry {
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
    pub features: Option<HashMap<String, Vec<String>>>,
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
    pub features2: Option<HashMap<String, Vec<String>>>,
    /// The minimal supported Rust version (optional)
    /// This must be a valid version requirement without an operator (e.g. no `=`)
    pub rust_version: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Dependency {
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
enum DependencyKind {
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

/// Get the path to the index file of the crate according to [Cargo's docs].
///
/// # Panics
///
/// The function will panic for empty names.
///
/// [Cargo's docs]: https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
fn index_url(name: &str) -> String {
    // the lint is about comparing to zero, but here we check if it's larger
    // than zero, which is more idiomatic in this case than `.is_empty()`.
    #[allow(clippy::len_zero)]
    {
        assert!(name.len() > 0);
    }

    let path = match name.len() {
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
    };

    format!("{REGISTRY_URL}/{path}")
}

#[inline]
fn api_url(name: &str) -> String {
    format!("{API_URL}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_url() {
        let prefix = format!("{REGISTRY_URL}/");

        assert_eq!(index_url("a").strip_prefix(&prefix).unwrap(), "1/a");
        assert_eq!(index_url("ab").strip_prefix(&prefix).unwrap(), "2/ab");
        assert_eq!(index_url("abc").strip_prefix(&prefix).unwrap(), "3/a/abc");
        assert_eq!(
            index_url("abcd").strip_prefix(&prefix).unwrap(),
            "ab/cd/abcd"
        );
        assert_eq!(
            index_url("cargo").strip_prefix(&prefix).unwrap(),
            "ca/rg/cargo"
        );
    }

    #[tokio::test]
    async fn test_working_fetch() {
        RegistryCache::new().fetch("base64").await.unwrap();
    }

    #[tokio::test]
    async fn test_failing_fetch() {
        RegistryCache::new()
            .fetch("my_name_is_inigo_montoya_and_there_is_no_way_there_is_a_crate_with_this_name")
            .await
            .unwrap_err();
    }
}
