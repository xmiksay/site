//! Storage backend settings: `STORAGE_KIND` (`db` | `fs` | `s3`),
//! `STORAGE_DIR` for `fs`, `S3_*` for `s3`.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

pub const DEFAULT_STORAGE_DIR: &str = "./data";
pub const DEFAULT_S3_REGION: &str = "us-east-1";

#[derive(Debug, Clone, PartialEq, Default)]
pub enum StorageConfig {
    /// Bytes in `file_blobs.data` — the original behavior, hence the default.
    #[default]
    Db,
    Fs {
        dir: PathBuf,
    },
    S3(S3Config),
}

#[derive(Clone, PartialEq)]
pub struct S3Config {
    /// `None` → AWS.
    pub endpoint: Option<String>,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub path_style: bool,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .finish()
    }
}

/// Empty and whitespace-only values mean "unset": container runtimes pass
/// unset variables through as empty strings.
fn set(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_bool(name: &str, v: Option<String>) -> Result<bool> {
    match set(v).map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("false" | "0" | "no") => Ok(false),
        Some("true" | "1" | "yes") => Ok(true),
        Some(other) => bail!("{name} must be true or false, not {other:?}"),
    }
}

impl StorageConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// `S3_*` variables under `prefix` (`""` for the app, `"TEST_"` for the
    /// test bucket).
    pub fn s3_from_env(prefix: &str) -> Result<S3Config> {
        S3Config::from_lookup(|name| std::env::var(format!("{prefix}{name}")).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        match set(get("STORAGE_KIND")).as_deref().unwrap_or("db") {
            "db" => Ok(Self::Db),
            "fs" => Ok(Self::Fs {
                dir: PathBuf::from(
                    set(get("STORAGE_DIR")).unwrap_or_else(|| DEFAULT_STORAGE_DIR.to_string()),
                ),
            }),
            "s3" => Ok(Self::S3(S3Config::from_lookup(get)?)),
            other => bail!("STORAGE_KIND must be db, fs or s3, not {other:?}"),
        }
    }
}

impl S3Config {
    /// The endpoint requests go to. Path-style: as configured. Virtual-hosted:
    /// the bucket goes into the host (`https://s3.example` →
    /// `https://site.s3.example`) unless the host already starts with it.
    pub fn request_endpoint(&self) -> Option<String> {
        let endpoint = self.endpoint.as_deref()?.trim_end_matches('/');
        if self.path_style {
            return Some(endpoint.to_string());
        }
        let prefix = format!("{}.", self.bucket);
        match endpoint.split_once("://") {
            Some((scheme, host)) if !host.starts_with(&prefix) => {
                Some(format!("{scheme}://{prefix}{host}"))
            }
            _ => Some(endpoint.to_string()),
        }
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let endpoint = set(get("S3_ENDPOINT"));
        if let Some(e) = &endpoint
            && !(e.starts_with("http://") || e.starts_with("https://"))
        {
            bail!("S3_ENDPOINT must start with http:// or https://");
        }
        Ok(Self {
            endpoint,
            bucket: set(get("S3_BUCKET")).context("S3_BUCKET is required for s3 storage")?,
            region: set(get("S3_REGION")).unwrap_or_else(|| DEFAULT_S3_REGION.to_string()),
            access_key_id: set(get("S3_ACCESS_KEY_ID"))
                .context("S3_ACCESS_KEY_ID is required for s3 storage")?,
            secret_access_key: set(get("S3_SECRET_ACCESS_KEY"))
                .context("S3_SECRET_ACCESS_KEY is required for s3 storage")?,
            path_style: parse_bool("S3_PATH_STYLE", get("S3_PATH_STYLE"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Result<StorageConfig> {
        StorageConfig::from_lookup(|name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        })
    }

    const S3: [(&str, &str); 6] = [
        ("STORAGE_KIND", "s3"),
        ("S3_ENDPOINT", "https://s3.example.test"),
        ("S3_BUCKET", "site"),
        ("S3_REGION", "garage"),
        ("S3_ACCESS_KEY_ID", "GKkey"),
        ("S3_SECRET_ACCESS_KEY", "s3cret"),
    ];

    #[test]
    fn db_is_the_default() {
        assert_eq!(cfg(&[]).expect("default"), StorageConfig::Db);
        assert_eq!(
            cfg(&[("STORAGE_KIND", " ")]).expect("blank"),
            StorageConfig::Db
        );
    }

    #[test]
    fn fs_dir_defaults_and_overrides() {
        let fs = cfg(&[("STORAGE_KIND", "fs"), ("STORAGE_DIR", "")]).expect("fs");
        assert_eq!(
            fs,
            StorageConfig::Fs {
                dir: PathBuf::from(DEFAULT_STORAGE_DIR)
            }
        );
        let custom = cfg(&[("STORAGE_KIND", "fs"), ("STORAGE_DIR", "/srv/blobs")]).expect("fs");
        assert_eq!(
            custom,
            StorageConfig::Fs {
                dir: PathBuf::from("/srv/blobs")
            }
        );
    }

    #[test]
    fn s3_settings_are_read_and_the_secret_is_redacted() {
        let mut pairs = S3.to_vec();
        pairs.push(("S3_PATH_STYLE", "true"));
        let StorageConfig::S3(s3) = cfg(&pairs).expect("s3") else {
            panic!("expected s3");
        };
        assert_eq!(s3.endpoint.as_deref(), Some("https://s3.example.test"));
        assert_eq!((s3.bucket.as_str(), s3.region.as_str()), ("site", "garage"));
        assert_eq!(s3.access_key_id, "GKkey");
        assert!(s3.path_style);
        assert!(!format!("{s3:?}").contains("s3cret"));
    }

    #[test]
    fn s3_defaults() {
        let pairs: Vec<_> = S3
            .iter()
            .copied()
            .filter(|(k, _)| !k.ends_with("ENDPOINT") && !k.ends_with("REGION"))
            .collect();
        let StorageConfig::S3(s3) = cfg(&pairs).expect("s3") else {
            panic!("expected s3");
        };
        assert_eq!(s3.endpoint, None);
        assert_eq!(s3.region, DEFAULT_S3_REGION);
        assert!(!s3.path_style);
    }

    #[test]
    fn virtual_hosted_endpoint_carries_the_bucket() {
        let StorageConfig::S3(mut s3) = cfg(&S3).expect("s3") else {
            panic!("expected s3");
        };
        assert_eq!(
            s3.request_endpoint().as_deref(),
            Some("https://site.s3.example.test")
        );
        s3.endpoint = Some("https://site.s3.example.test/".into());
        assert_eq!(
            s3.request_endpoint().as_deref(),
            Some("https://site.s3.example.test")
        );
        s3.path_style = true;
        s3.endpoint = Some("http://127.0.0.1:3900".into());
        assert_eq!(
            s3.request_endpoint().as_deref(),
            Some("http://127.0.0.1:3900")
        );
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let err = cfg(&[("STORAGE_KIND", "ftp")]).expect_err("kind");
        assert!(err.to_string().contains("STORAGE_KIND"), "{err}");
        for missing in ["BUCKET", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY"] {
            let pairs: Vec<_> = S3
                .iter()
                .copied()
                .filter(|(k, _)| !k.ends_with(missing))
                .collect();
            let err = cfg(&pairs).expect_err(missing);
            assert!(format!("{err:#}").contains(missing), "{err:#}");
        }
        let mut pairs = S3.to_vec();
        pairs.push(("S3_PATH_STYLE", "maybe"));
        assert!(cfg(&pairs).is_err());
        let mut pairs = S3.to_vec();
        pairs[1] = ("S3_ENDPOINT", "s3.example.test");
        let err = cfg(&pairs).expect_err("endpoint scheme");
        assert!(err.to_string().contains("S3_ENDPOINT"), "{err}");
    }
}
