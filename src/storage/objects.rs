//! Keyed objects next to the content-addressed blobs (design overrides live
//! under `design/…`). Every backend has them: `fs`/`s3` as object keys, `db`
//! as `storage_objects` rows (see [`db_objects`](super::db_objects)).

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt as _;
use object_store::ObjectStoreExt as _;
use object_store::path::Path;

use super::{BLOB_PREFIX, Error, Storage, db_objects};

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
    /// Store `bytes` under `key`, replacing it. Readers see the old object or
    /// the new one, never a partial write.
    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<(), Error> {
        let path = parse_key(key)?;
        let Some(objects) = &self.objects else {
            return Ok(db_objects::put(&self.db, &self.db_key(key), &bytes).await?);
        };
        objects
            .store
            .put(&path, bytes.into())
            .await
            .map(|_| ())
            .map_err(Error::Unavailable)
    }

    /// The object's bytes, `None` when missing.
    pub async fn get(&self, key: &str) -> Result<Option<Bytes>, Error> {
        let path = parse_key(key)?;
        let Some(objects) = &self.objects else {
            return Ok(db_objects::get(&self.db, &self.db_key(key)).await?);
        };
        let got = match objects.store.get(&path).await {
            Ok(got) => got,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(Error::Unavailable(e)),
        };
        got.bytes().await.map(Some).map_err(Error::Unavailable)
    }

    /// Idempotent: a missing object is not an error.
    pub async fn delete(&self, key: &str) -> Result<(), Error> {
        let path = parse_key(key)?;
        let Some(objects) = &self.objects else {
            return Ok(db_objects::delete(&self.db, &self.db_key(key)).await?);
        };
        match objects.store.delete(&path).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(Error::Unavailable(e)),
        }
    }

    /// Every object under the directory `prefix` (no trailing `/`),
    /// recursively, sorted by key.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Object>, Error> {
        let path = parse_key(prefix)?;
        self.list_under(Some(&path)).await
    }

    /// Every keyed object, i.e. everything but the blobs, sorted by key —
    /// what `storage migrate` copies.
    pub async fn list_all(&self) -> Result<Vec<Object>, Error> {
        let blobs = format!("{BLOB_PREFIX}/");
        let mut all = self.list_under(None).await?;
        all.retain(|o| !o.key.starts_with(&blobs));
        Ok(all)
    }

    async fn list_under(&self, dir: Option<&Path>) -> Result<Vec<Object>, Error> {
        let mut out: Vec<Object> = match &self.objects {
            Some(objects) => objects
                .store
                .list(dir)
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
                .map_err(Error::Unavailable)?,
            None => {
                let under = dir.map(|d| format!("{d}/")).unwrap_or_default();
                db_objects::list(&self.db, &self.db_key(&under))
                    .await?
                    .into_iter()
                    .filter_map(|mut o| {
                        o.key = o.key.strip_prefix(&self.key_prefix)?.to_string();
                        Some(o)
                    })
                    .collect()
            }
        };
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    /// The `storage_objects` key of `key` under this storage's `scoped`
    /// prefix (object backends wrap their store in a `PrefixStore` instead).
    fn db_key(&self, key: &str) -> String {
        format!("{}{key}", self.key_prefix)
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

    #[test]
    fn scoped_db_keys_nest_under_each_prefix() {
        let db = sea_orm::DatabaseConnection::Disconnected;
        let storage = Storage::db(db).scoped("test-1").scoped("inner");
        assert_eq!(storage.db_key("design/x.css"), "test-1/inner/design/x.css");
        assert_eq!(
            Storage::db(sea_orm::DatabaseConnection::Disconnected).db_key("a/b"),
            "a/b"
        );
    }
}
