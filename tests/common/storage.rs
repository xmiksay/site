//! Storage backends for tests: a random key prefix in `storage_objects`
//! (`db`; blobs are shared `file_blobs` rows), a throwaway directory (`fs`) or
//! a random key prefix in the S3 test bucket (`s3`, from `TEST_S3_*`), each
//! removed on drop. Included via `#[path]` only by the
//! binaries that need it (like `scripted.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::TryStreamExt as _;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt as _};
use sea_orm::{ConnectionTrait as _, Database, DatabaseBackend, DatabaseConnection, Statement};
use site::storage::{S3Config, Storage, StorageConfig};

const S3_VARS: [&str; 5] = [
    "TEST_S3_ENDPOINT",
    "TEST_S3_BUCKET",
    "TEST_S3_REGION",
    "TEST_S3_ACCESS_KEY_ID",
    "TEST_S3_SECRET_ACCESS_KEY",
];

/// The S3 test bucket. Fails, not skips, without `TEST_S3_*`: a silently
/// skipped S3 suite would let the production backend go untested.
pub fn s3_config() -> S3Config {
    for name in S3_VARS {
        assert!(
            std::env::var(name).is_ok_and(|v| !v.trim().is_empty()),
            "{name} must be set for the S3 storage tests (all of {S3_VARS:?}; see .env.example)"
        );
    }
    let mut cfg = StorageConfig::s3_from_env("TEST_").expect("TEST_S3_* configuration");
    cfg.path_style = true;
    cfg
}

/// An S3 config whose endpoint refuses connections.
pub fn dead_s3_config() -> S3Config {
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port");
    S3Config {
        endpoint: Some(format!("http://{addr}")),
        bucket: "site-test".into(),
        region: "garage".into(),
        access_key_id: "GKdead".into(),
        secret_access_key: "dead".into(),
        path_style: true,
    }
}

pub struct TestStorage {
    pub storage: Storage,
    /// The fs root to remove on drop.
    dir: Option<PathBuf>,
    /// The S3 prefix to delete on drop.
    s3_prefix: Option<String>,
    /// The `storage_objects` key prefix to delete on drop.
    db_prefix: Option<String>,
}

impl TestStorage {
    pub fn db(db: &DatabaseConnection) -> Self {
        let prefix = format!("test-{}", uuid::Uuid::new_v4());
        Self {
            storage: Storage::db(db.clone()).scoped(&prefix),
            dir: None,
            s3_prefix: None,
            db_prefix: Some(prefix),
        }
    }

    pub fn fs(db: &DatabaseConnection) -> Self {
        let dir = std::env::temp_dir().join(format!("site-storage-{}", uuid::Uuid::new_v4()));
        let storage = Storage::local(&dir, db.clone()).expect("fs storage");
        Self {
            storage,
            dir: Some(dir),
            s3_prefix: None,
            db_prefix: None,
        }
    }

    pub fn s3(db: &DatabaseConnection) -> Self {
        let prefix = format!("test-{}", uuid::Uuid::new_v4());
        let storage = Storage::s3(&s3_config(), db.clone())
            .expect("S3 test storage")
            .scoped(&prefix);
        Self {
            storage,
            dir: None,
            s3_prefix: Some(prefix),
            db_prefix: None,
        }
    }

    /// The equivalent `StorageConfig`, for building a full `AppState` over
    /// the same place (fs only — an S3 state cannot be scoped).
    pub fn fs_config(&self) -> StorageConfig {
        StorageConfig::Fs {
            dir: self.dir.clone().expect("an fs test storage"),
        }
    }
}

impl Drop for TestStorage {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(prefix) = self.db_prefix.take() {
            delete_db_objects(prefix);
        }
        let Some(prefix) = self.s3_prefix.take() else {
            return;
        };
        // Dedicated thread + runtime: works under any test runtime flavor,
        // with a fresh client — the test's pooled connections belong to its
        // (now blocked) runtime.
        let _ = std::thread::spawn(move || {
            let cfg = s3_config();
            let Ok(store) = AmazonS3Builder::new()
                .with_bucket_name(&cfg.bucket)
                .with_region(&cfg.region)
                .with_access_key_id(&cfg.access_key_id)
                .with_secret_access_key(&cfg.secret_access_key)
                .with_endpoint(cfg.endpoint.clone().unwrap_or_default())
                .with_allow_http(true)
                .build()
            else {
                return;
            };
            let store: Arc<dyn ObjectStore> = Arc::new(store);
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let Ok(objects) = store
                    .list(Some(&Path::from(prefix)))
                    .try_collect::<Vec<_>>()
                    .await
                else {
                    return;
                };
                for o in objects {
                    let _ = store.delete(&o.location).await;
                }
            });
        })
        .join();
    }
}

/// Same dedicated-thread pattern as the S3 cleanup above, with a fresh
/// connection.
fn delete_db_objects(prefix: String) {
    let _ = std::thread::spawn(move || {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok(db) = Database::connect(&url).await else {
                return;
            };
            let stmt = Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM storage_objects WHERE starts_with(key, $1)",
                [format!("{prefix}/").into()],
            );
            let _ = db.execute(stmt).await;
        });
    })
    .join();
}
