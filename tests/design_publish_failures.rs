//! Failure paths of a design publish (#115): the startup recovery of a
//! publish interrupted after its pending marker (db, fs), and a mirror
//! failing midway restoring the previous `design/` (fs, via a read-only
//! directory). The happy path is in `tests/design_publish.rs`.
//!
//! Gated on `DATABASE_URL` like every DB test.

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use std::sync::Arc;

use bytes::Bytes;
use sea_orm::{Database, DatabaseConnection};
use site::design::DesignStore;
use site::design::stored::DesignError;
use site::storage::{Storage, StorageConfig};
use site::templates::Templates;
use storage_fixture::TestStorage;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn setup() -> (Arc<DesignStore>, Templates) {
    let design = Arc::new(DesignStore::new(None));
    let tmpl = Templates::new(design.clone());
    (design, tmpl)
}

async fn put(design: &DesignStore, storage: &Storage, path: &str, body: &str) {
    design
        .draft_put(storage, path, Bytes::from(body.to_string()))
        .await
        .expect("draft put");
}

async fn changes(design: &DesignStore, storage: &Storage) -> Vec<String> {
    let draft = design.draft(storage).await.expect("draft");
    draft.changes.into_iter().map(|c| c.path).collect()
}

/// A crash after the snapshot and the pending marker: the next start rolls
/// the snapshot forward; a marker whose snapshot is incomplete is dropped
/// without touching `design/`.
async fn exercise_recovery(ts: &TestStorage) {
    let storage = &ts.storage;
    let id = "2026-10-10T12:00:00.000000Z";
    storage
        .put("design/assets/css/old.css", "old".into())
        .await
        .expect("put");
    storage
        .put(
            &format!("design-history/{id}/templates/404.html"),
            "ROLLED".into(),
        )
        .await
        .expect("put");
    let marker = format!(r#"{{"id":"{id}","at":"{id}","by":"carol","files":1}}"#);
    storage
        .put("design-publish-pending.json", marker.into())
        .await
        .expect("put");

    let (design, tmpl) = setup();
    let entry = design.recover_publish(storage).await.expect("recover");
    assert_eq!(entry.map(|e| e.by).as_deref(), Some("carol"));
    design.reload(storage, &tmpl).await.expect("reload");
    assert_eq!(
        design.load("templates/404.html").as_deref(),
        Some(&b"ROLLED"[..])
    );
    assert!(
        design.load("assets/css/old.css").is_none(),
        "mirrored exactly"
    );
    assert_eq!(design.history(storage).await.expect("history").len(), 1);
    assert!(
        design
            .recover_publish(storage)
            .await
            .expect("noop")
            .is_none()
    );

    let marker = r#"{"id":"2026-10-10T13:00:00Z","at":"2026-10-10T13:00:00Z","by":"x","files":3}"#;
    storage
        .put("design-publish-pending.json", marker.into())
        .await
        .expect("put");
    assert!(
        design
            .recover_publish(storage)
            .await
            .expect("drop")
            .is_none()
    );
    assert!(
        storage
            .get("design-publish-pending.json")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        storage
            .get("design/templates/404.html")
            .await
            .expect("get")
            .is_some()
    );
}

#[tokio::test]
async fn recover_interrupted_publish_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_recovery(&TestStorage::db(&db)).await;
}

#[tokio::test]
async fn recover_interrupted_publish_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_recovery(&TestStorage::fs(&db)).await;
}

/// A mirror that fails midway (a read-only directory under `design/`) puts
/// the previous `design/` objects back and keeps the running design.
#[cfg(unix)]
#[tokio::test]
async fn failed_mirror_restores_the_previous_design_over_fs() {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let storage = &ts.storage;
    let (design, tmpl) = setup();
    design.reload(storage, &tmpl).await.expect("reload");
    put(&design, storage, "assets/css/style.css", "v1{}").await;
    design
        .publish(storage, &tmpl, "alice")
        .await
        .expect("publish");

    let StorageConfig::Fs { dir } = ts.fs_config() else {
        unreachable!()
    };
    let locked = dir.join("design/assets/locked");
    std::fs::create_dir_all(&locked).expect("mkdir");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    if std::fs::write(locked.join("probe"), "x").is_ok() {
        eprintln!("skipping: running as root, permissions are not enforced");
        return;
    }

    // Sorted mirror order: style.css is written before locked/x.css fails.
    put(&design, storage, "assets/css/style.css", "v2{}").await;
    put(&design, storage, "assets/locked/x.css", "x{}").await;
    let err = design
        .publish(storage, &tmpl, "alice")
        .await
        .expect_err("mirror fails");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        matches!(err, DesignError::PublishFailed { restored: true, .. }),
        "{err:?}"
    );
    let css = storage
        .get("design/assets/css/style.css")
        .await
        .expect("get");
    assert_eq!(
        css.as_deref(),
        Some(&b"v1{}"[..]),
        "previous design/ restored"
    );
    assert_eq!(
        design.load("assets/css/style.css").as_deref(),
        Some(&b"v1{}"[..])
    );
    assert!(
        storage
            .get("design-publish-pending.json")
            .await
            .expect("get")
            .is_none()
    );
    assert_eq!(design.history(storage).await.expect("history").len(), 1);
    // The draft keeps the unpublished edit.
    assert_eq!(changes(&design, storage).await.len(), 2);
}
