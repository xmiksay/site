//! Content-addressed blob storage for file and thumbnail bytes. One
//! [`Storage`] over three backends: `db` (bytes in `file_blobs.data`), `fs`
//! (a local directory) or `s3` (an S3-compatible bucket), the latter two via
//! the `object_store` crate.
//!
//! Every backend keeps a `file_blobs` row per blob (hash, size): the
//! `files`/`file_thumbnails` foreign keys point at it, and `storage migrate`
//! walks it. Only the `db` backend fills its `data` column. Object keys are
//! `blobs/{hash[0..2]}/{hash}`. Every backend also holds keyed objects
//! (design overrides under `design/…`, see [`objects`]): object keys on
//! `fs`/`s3`, `storage_objects` rows on `db`.

pub mod config;
mod db_objects;
pub mod migrate;
mod objects;

use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use futures_util::stream::{self, BoxStream};
use futures_util::{StreamExt as _, TryStreamExt as _};
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::{ClientOptions, ObjectStore, ObjectStoreExt as _, RetryConfig};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
};

use crate::files::hash_blob;
pub use config::{S3Config, StorageConfig};
pub use objects::{Object, Version, parse_key};

/// Bounds how long a request waits on an unreachable bucket before it fails
/// with 503 (the crate's default retries for minutes). No total timeout: it
/// would also cut off a slow but progressing download stream; the read
/// timeout bounds each wait for headers or the next chunk.
const S3_READ_TIMEOUT: Duration = Duration::from_secs(30);
const S3_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const S3_RETRY_TIMEOUT: Duration = Duration::from_secs(15);
const S3_MAX_RETRIES: usize = 2;

const BLOB_PREFIX: &str = "blobs";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid blob hash {0:?}")]
    InvalidHash(String),
    #[error("invalid storage key {0:?}")]
    InvalidKey(String),
    #[error("storage unavailable: {0}")]
    Unavailable(#[source] object_store::Error),
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// A blob's body as a stream (downloads), with its size.
pub struct Download {
    pub size: u64,
    pub stream: BoxStream<'static, Result<Bytes, Error>>,
}

#[derive(Clone)]
pub struct Storage {
    db: DatabaseConnection,
    objects: Option<Objects>,
    /// The `scoped` prefixes, each ending in `/`, prepended to every
    /// `storage_objects` key. Set on every backend but used only by `db`
    /// (`fs`/`s3` scope through a `PrefixStore`).
    key_prefix: String,
}

#[derive(Clone)]
struct Objects {
    store: Arc<dyn ObjectStore>,
    kind: &'static str,
}

impl Storage {
    /// The configured backend. `fs` creates its directory; nothing touches
    /// the network here.
    pub fn new(cfg: &StorageConfig, db: DatabaseConnection) -> anyhow::Result<Self> {
        match cfg {
            StorageConfig::Db => Ok(Self::db(db)),
            StorageConfig::Fs { dir } => Self::local(dir, db),
            StorageConfig::S3(s3) => Self::s3(s3, db),
        }
    }

    pub fn db(db: DatabaseConnection) -> Self {
        Self {
            db,
            objects: None,
            key_prefix: String::new(),
        }
    }

    /// A directory: atomic writes (temp file + rename) with fsync.
    pub fn local(dir: &FsPath, db: DatabaseConnection) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create STORAGE_DIR {}", dir.display()))?;
        let store = LocalFileSystem::new_with_prefix(dir)
            .with_context(|| format!("open STORAGE_DIR {}", dir.display()))?
            .with_fsync(true);
        Ok(Self::with_store(Arc::new(store), "fs", db))
    }

    pub fn s3(cfg: &S3Config, db: DatabaseConnection) -> anyhow::Result<Self> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&cfg.bucket)
            .with_region(&cfg.region)
            .with_access_key_id(&cfg.access_key_id)
            .with_secret_access_key(&cfg.secret_access_key)
            .with_virtual_hosted_style_request(!cfg.path_style)
            .with_client_options(s3_client_options())
            .with_retry(RetryConfig {
                backoff: Default::default(),
                max_retries: S3_MAX_RETRIES,
                retry_timeout: S3_RETRY_TIMEOUT,
            });
        if let Some(endpoint) = cfg.request_endpoint() {
            builder = builder
                .with_allow_http(endpoint.starts_with("http://"))
                .with_endpoint(endpoint);
        }
        let store = builder.build().context("configure the S3 storage")?;
        Ok(Self::with_store(Arc::new(store), "s3", db))
    }

    fn with_store(store: Arc<dyn ObjectStore>, kind: &'static str, db: DatabaseConnection) -> Self {
        Self {
            db,
            objects: Some(Objects { store, kind }),
            key_prefix: String::new(),
        }
    }

    /// `db`, `fs` or `s3`.
    pub fn kind(&self) -> &'static str {
        self.objects.as_ref().map_or("db", |o| o.kind)
    }

    /// The same backend with every object key under `prefix/` (tests isolate
    /// themselves in one bucket or table this way). Blobs stay shared on `db`:
    /// they are content-addressed `file_blobs` rows.
    pub fn scoped(&self, prefix: &str) -> Self {
        let objects = self.objects.as_ref().map(|o| Objects {
            store: Arc::new(PrefixStore::new(Arc::clone(&o.store), prefix)),
            kind: o.kind,
        });
        Self {
            db: self.db.clone(),
            objects,
            key_prefix: format!("{}{prefix}/", self.key_prefix),
        }
    }

    /// Store `data` and return its sha256 hash. Re-storing an existing blob is
    /// a no-op apart from the (idempotent) write itself.
    pub async fn put_blob(&self, data: &[u8]) -> Result<String, Error> {
        let hash = hash_blob(data);
        let size = data.len() as i64;
        let Some(objects) = &self.objects else {
            // A metadata-only row (left by an object backend) gets its bytes.
            self.exec(
                "INSERT INTO file_blobs (hash, data, size_bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (hash) DO UPDATE SET data = EXCLUDED.data
                 WHERE file_blobs.data IS NULL",
                [hash.clone().into(), data.to_vec().into(), size.into()],
            )
            .await?;
            return Ok(hash);
        };
        // Object first: a row must never point at bytes that were not written.
        objects
            .store
            .put(&blob_path(&hash)?, Bytes::copy_from_slice(data).into())
            .await
            .map_err(Error::Unavailable)?;
        self.exec(
            "INSERT INTO file_blobs (hash, size_bytes) VALUES ($1, $2)
             ON CONFLICT (hash) DO NOTHING",
            [hash.clone().into(), size.into()],
        )
        .await?;
        Ok(hash)
    }

    /// The blob's bytes, `None` when this backend does not hold it.
    pub async fn get_blob(&self, hash: &str) -> Result<Option<Bytes>, Error> {
        let Some(objects) = &self.objects else {
            return self.db_blob(hash).await;
        };
        let got = match objects.store.get(&blob_path(hash)?).await {
            Ok(got) => got,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(Error::Unavailable(e)),
        };
        got.bytes().await.map(Some).map_err(Error::Unavailable)
    }

    /// Streams the body; a missing blob is `None` before the first byte.
    pub async fn get_blob_stream(&self, hash: &str) -> Result<Option<Download>, Error> {
        let Some(objects) = &self.objects else {
            return Ok(self.db_blob(hash).await?.map(|data| Download {
                size: data.len() as u64,
                stream: stream::once(async move { Ok(data) }).boxed(),
            }));
        };
        match objects.store.get(&blob_path(hash)?).await {
            Ok(got) => Ok(Some(Download {
                size: got.meta.size,
                stream: got.into_stream().map_err(Error::Unavailable).boxed(),
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(Error::Unavailable(e)),
        }
    }

    /// Every blob hash the database knows about, sorted.
    pub async fn known_hashes(&self) -> Result<Vec<String>, DbErr> {
        #[derive(FromQueryResult)]
        struct Row {
            hash: String,
        }
        let stmt = Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT hash FROM file_blobs ORDER BY hash",
        );
        let rows = Row::find_by_statement(stmt).all(&self.db).await?;
        Ok(rows.into_iter().map(|r| r.hash).collect())
    }

    async fn db_blob(&self, hash: &str) -> Result<Option<Bytes>, Error> {
        #[derive(FromQueryResult)]
        struct Row {
            data: Option<Vec<u8>>,
        }
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT data FROM file_blobs WHERE hash = $1",
            [hash.into()],
        );
        let row = Row::find_by_statement(stmt).one(&self.db).await?;
        Ok(row.and_then(|r| r.data).map(Bytes::from))
    }

    async fn exec<const N: usize>(
        &self,
        sql: &str,
        values: [sea_orm::Value; N],
    ) -> Result<(), DbErr> {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values);
        self.db.execute(stmt).await.map(|_| ())
    }
}

fn s3_client_options() -> ClientOptions {
    ClientOptions::new()
        .with_timeout_disabled()
        .with_read_timeout(S3_READ_TIMEOUT)
        .with_connect_timeout(S3_CONNECT_TIMEOUT)
}

/// Only a lowercase sha256 hex digest names a blob, so a key built from one
/// can never escape the blob prefix.
fn blob_path(hash: &str) -> Result<Path, Error> {
    let valid = hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !valid {
        return Err(Error::InvalidHash(hash.to_string()));
    }
    Ok(Path::from(format!("{BLOB_PREFIX}/{}/{hash}", &hash[..2])))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn blob_keys_are_sharded_by_the_first_byte() {
        assert_eq!(
            blob_path(ABC).expect("valid").to_string(),
            format!("blobs/ba/{ABC}")
        );
    }

    #[test]
    fn only_sha256_digests_are_blob_keys() {
        let upper = ABC.to_ascii_uppercase();
        let short = &ABC[..63];
        let traversal = format!("../{}", &ABC[3..]);
        for bad in ["", short, upper.as_str(), traversal.as_str()] {
            assert!(
                matches!(blob_path(bad), Err(Error::InvalidHash(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn s3_bodies_have_no_total_timeout() {
        use object_store::ClientConfigKey as K;
        let opts = s3_client_options();
        assert_eq!(opts.get_config_value(&K::Timeout), None);
        assert!(opts.get_config_value(&K::ReadTimeout).is_some());
        assert!(opts.get_config_value(&K::ConnectTimeout).is_some());
    }
}
