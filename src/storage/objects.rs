//! Keyed objects next to the content-addressed blobs: design overrides live
//! under `design/…`. Only the object backends (`fs`, `s3`) have them; the `db`
//! backend answers [`Error::NoObjectStore`].

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt as _;
use object_store::ObjectStoreExt as _;
use object_store::path::Path;

use super::{Error, Objects, Storage};

/// What decides whether a cached copy is still current: the backend's ETag
/// plus size and modification time (a backend without ETags still changes one
/// of those on every write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub e_tag: Option<String>,
    pub size: u64,
    pub last_modified: DateTime<Utc>,
}

/// A listed object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub version: Version,
}

impl Storage {
    /// Whether keyed objects are available (`fs` / `s3`).
    pub fn has_objects(&self) -> bool {
        self.objects.is_some()
    }

    fn objects(&self) -> Result<&Objects, Error> {
        self.objects.as_ref().ok_or(Error::NoObjectStore)
    }

    /// Store `bytes` under `key`, replacing it. Readers see the old object or
    /// the new one, never a partial write.
    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<(), Error> {
        let path = parse_key(key)?;
        self.objects()?
            .store
            .put(&path, bytes.into())
            .await
            .map(|_| ())
            .map_err(Error::Unavailable)
    }

    /// The object's bytes, `None` when missing.
    pub async fn get(&self, key: &str) -> Result<Option<Bytes>, Error> {
        let path = parse_key(key)?;
        let got = match self.objects()?.store.get(&path).await {
            Ok(got) => got,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(Error::Unavailable(e)),
        };
        got.bytes().await.map(Some).map_err(Error::Unavailable)
    }

    /// Idempotent: a missing object is not an error.
    pub async fn delete(&self, key: &str) -> Result<(), Error> {
        let path = parse_key(key)?;
        match self.objects()?.store.delete(&path).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(Error::Unavailable(e)),
        }
    }

    /// Every object under the directory `prefix` (no trailing `/`),
    /// recursively, sorted by key.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Object>, Error> {
        let path = parse_key(prefix)?;
        let mut out: Vec<Object> = self
            .objects()?
            .store
            .list(Some(&path))
            .map_ok(|meta| Object {
                key: meta.location.to_string(),
                version: Version {
                    e_tag: meta.e_tag,
                    size: meta.size,
                    last_modified: meta.last_modified,
                },
            })
            .try_collect()
            .await
            .map_err(Error::Unavailable)?;
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}

/// Keys never contain empty, `.` or `..` segments or control characters —
/// they cannot escape the storage root.
pub fn parse_key(key: &str) -> Result<Path, Error> {
    Path::parse(key)
        .ok()
        .filter(|p| p.parts().next().is_some() && p.as_ref() == key)
        .ok_or_else(|| Error::InvalidKey(key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated() {
        assert!(parse_key("design/templates/base.html").is_ok());
        assert!(parse_key("design/assets/fonts/Brand Bold.ttf").is_ok());
        for bad in [
            "",
            "/design/x",
            "design/",
            "a//b",
            "../x",
            "a/./b",
            "a/../b",
            "a\nb",
        ] {
            assert!(
                matches!(parse_key(bad), Err(Error::InvalidKey(_))),
                "{bad:?}"
            );
        }
    }
}
