//! Failure paths of a design publish (#115): a publish left pending (its
//! marker set) completed by the next reload or publish, with the draft's
//! base moved along only when the draft is that snapshot (db, fs); files
//! added and deleted in `design/` outside the draft reported as conflicts
//! (db, fs); and a mirror failing midway restoring the previous `design/`
//! (fs, via a read-only directory). The happy path is in
//! `tests/design_publish.rs`.
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
use site::design::stored::{DesignError, Files};
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

fn one_file(body: &str) -> Files {
    Files::from([(
        "templates/404.html".to_string(),
        Bytes::from(body.to_string()),
    )])
}

/// Leave what a publish interrupted after its marker leaves: a snapshot of
/// `files` and a marker claiming `count` files.
async fn leave_pending(storage: &Storage, id: &str, files: &Files, count: usize) {
    for (path, bytes) in files {
        let key = format!("design-history/{id}/{path}");
        storage.put(&key, bytes.clone()).await.expect("put");
    }
    let marker = format!(r#"{{"id":"{id}","at":"{id}","by":"carol","files":{count}}}"#);
    storage
        .put("design-publish-pending.json", marker.into())
        .await
        .expect("put");
}

async fn pending(storage: &Storage) -> bool {
    let marker = storage.get("design-publish-pending.json").await;
    marker.expect("get").is_some()
}

fn conflict_paths(err: DesignError) -> Vec<String> {
    match err {
        DesignError::Conflict(paths) => paths,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

/// A marker left behind (crash, failed restore) is completed by the next
/// reload or publish of a running store, not only at start: its snapshot is
/// rolled forward and recorded. The draft moves its base along only if it
/// still is that snapshot; otherwise the rolled-forward paths surface as a
/// conflict. A marker whose snapshot is incomplete is dropped without
/// touching `design/`.
async fn exercise_recovery(ts: &TestStorage) {
    let storage = &ts.storage;
    let (design, tmpl) = setup();
    design.reload(storage, &tmpl).await.expect("reload");
    put(&design, storage, "assets/css/a.css", "draft{}").await;

    // By a reload, which reports it; a half-mirrored leftover is removed.
    storage
        .put("design/assets/css/old.css", "old".into())
        .await
        .expect("put");
    let id = "2026-10-10T12:00:00.000000Z";
    leave_pending(storage, id, &one_file("ROLLED"), 1).await;
    let status = design.reload(storage, &tmpl).await.expect("reload");
    assert_eq!(status.completed_publish.map(|e| e.id).as_deref(), Some(id));
    assert!(!pending(storage).await);
    assert_eq!(
        design.load("templates/404.html").as_deref(),
        Some(&b"ROLLED"[..])
    );
    assert!(
        design.load("assets/css/old.css").is_none(),
        "mirrored exactly"
    );

    // By a publish. The draft (not the snapshot) kept its base, so
    // publishing it would revert the rolled-forward 404: a conflict.
    let id2 = "2026-10-10T13:00:00.000000Z";
    leave_pending(storage, id2, &one_file("ROLLED-2"), 1).await;
    let err = design
        .publish(storage, &tmpl, "dave", false)
        .await
        .expect_err("rolled-forward paths conflict");
    assert_eq!(conflict_paths(err), ["templates/404.html"]);
    assert!(!pending(storage).await);
    assert_eq!(
        design.load("templates/404.html").as_deref(),
        Some(&b"ROLLED-2"[..])
    );
    assert_eq!(design.history(storage).await.expect("history").len(), 2);
    design
        .publish(storage, &tmpl, "dave", true)
        .await
        .expect("forced");

    // A draft that still is the snapshot moves its base along: no conflict.
    put(&design, storage, "assets/css/a.css", "v3{}").await;
    let draft = design.draft(storage).await.expect("draft").files;
    leave_pending(storage, "2026-10-10T14:00:00.000000Z", &draft, draft.len()).await;
    design.reload(storage, &tmpl).await.expect("reload");
    put(&design, storage, "assets/css/b.css", "b{}").await;
    design
        .publish(storage, &tmpl, "dave", false)
        .await
        .expect("no conflict after a re-base");
    assert_eq!(design.history(storage).await.expect("history").len(), 5);

    let before = storage.list("design").await.expect("list").len();
    let partial = "2026-10-10T15:00:00Z";
    leave_pending(storage, partial, &one_file("PARTIAL"), 3).await;
    let status = design.reload(storage, &tmpl).await.expect("reload");
    assert!(status.completed_publish.is_none());
    assert!(!pending(storage).await);
    assert_eq!(storage.list("design").await.expect("list").len(), before);
}

/// `design/` files added and deleted outside the draft are both conflicts.
async fn exercise_external_add_and_delete(ts: &TestStorage) {
    let storage = &ts.storage;
    let (design, tmpl) = setup();
    design.reload(storage, &tmpl).await.expect("reload");
    put(&design, storage, "assets/css/mine.css", "m{}").await;
    design
        .publish(storage, &tmpl, "erin", false)
        .await
        .expect("publish");

    storage
        .put("design/assets/css/added.css", "a{}".into())
        .await
        .expect("bucket add");
    storage
        .delete("design/assets/css/mine.css")
        .await
        .expect("bucket delete");
    put(&design, storage, "assets/css/next.css", "n{}").await;
    let err = design
        .publish(storage, &tmpl, "erin", false)
        .await
        .expect_err("conflict");
    assert_eq!(
        conflict_paths(err),
        ["assets/css/added.css", "assets/css/mine.css"]
    );
}

#[tokio::test]
async fn external_add_and_delete_conflict_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_external_add_and_delete(&TestStorage::db(&db)).await;
}

#[tokio::test]
async fn external_add_and_delete_conflict_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_external_add_and_delete(&TestStorage::fs(&db)).await;
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
        .publish(storage, &tmpl, "alice", false)
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
        .publish(storage, &tmpl, "alice", false)
        .await
        .expect_err("mirror fails");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        matches!(
            err,
            DesignError::PublishFailed {
                restored: true,
                pending: false,
                ..
            }
        ),
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
