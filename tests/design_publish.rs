//! The shared design draft, publish and version history (#115) over db, fs
//! and S3: draft init, edits invisible until publish, a failed validation
//! leaving live untouched, mirror deletions, history, restore into the
//! draft, discard. The failure paths (crash recovery, mirror rollback) are in
//! `tests/design_publish_failures.rs`.
//!
//! Gated on `DATABASE_URL` like every DB test; the S3 test fails, not skips,
//! without `TEST_S3_*` (see `common/storage.rs`).

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use std::sync::Arc;

use bytes::Bytes;
use sea_orm::{Database, DatabaseConnection};
use site::design::DesignStore;
use site::design::draft::ChangeKind;
use site::design::stored::DesignError;
use site::storage::Storage;
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

async fn changes(design: &DesignStore, storage: &Storage) -> Vec<(String, ChangeKind)> {
    let draft = design.draft(storage).await.expect("draft");
    draft
        .changes
        .into_iter()
        .map(|c| (c.path, c.kind))
        .collect()
}

fn live(design: &DesignStore, path: &str) -> Option<Vec<u8>> {
    design.load(path)
}

async fn exercise(ts: &TestStorage) {
    let (design, tmpl) = setup();
    let storage = &ts.storage;
    design.reload(storage, &tmpl).await.expect("reload");
    let baked_404 = design.baked("templates/404.html").expect("baked 404");

    // First access copies the published view (here: baked) into the draft.
    let draft = design.draft(storage).await.expect("draft init");
    assert!(draft.changes.is_empty(), "{:?}", draft.changes);
    assert_eq!(draft.files, design.published_view());
    let stored = storage.get("design-draft/templates/404.html").await;
    assert_eq!(
        stored.expect("get").map(|b| b.to_vec()),
        Some(baked_404.clone())
    );

    // Draft edits are invisible live.
    put(&design, storage, "templates/404.html", "DRAFT-1").await;
    put(&design, storage, "assets/css/extra.css", "a{}").await;
    let binary = Bytes::from_static(&[0, 159, 146, 150]);
    design
        .draft_put(storage, "assets/img/bin.png", binary.clone())
        .await
        .expect("binary put");
    assert_eq!(live(&design, "templates/404.html"), Some(baked_404.clone()));
    assert_eq!(
        changes(&design, storage).await,
        [
            ("assets/css/extra.css".to_string(), ChangeKind::Added),
            ("assets/img/bin.png".to_string(), ChangeKind::Added),
            ("templates/404.html".to_string(), ChangeKind::Modified),
        ]
    );
    let read = design.draft_read(storage, "assets/img/bin.png").await;
    assert_eq!(read.expect("read"), Some(binary));
    design
        .draft_delete(storage, "assets/img/bin.png")
        .await
        .expect("delete");
    let err = design
        .draft_delete(storage, "assets/img/bin.png")
        .await
        .expect_err("second delete");
    assert!(matches!(err, DesignError::NotInDraft(_)), "{err:?}");
    let err = design
        .draft_put(storage, "preview/x.html", Bytes::new())
        .await
        .expect_err("bad path");
    assert!(matches!(err, DesignError::BadPath(_)), "{err:?}");

    // A broken template fails validation: nothing written, nothing live.
    put(&design, storage, "templates/404.html", "{% if %}").await;
    let err = design
        .publish(storage, &tmpl, "alice")
        .await
        .expect_err("broken publish");
    assert!(matches!(err, DesignError::Invalid(_)), "{err:?}");
    assert!(storage.list("design").await.expect("list").is_empty());
    assert!(
        storage
            .list("design-history")
            .await
            .expect("list")
            .is_empty()
    );
    assert_eq!(live(&design, "templates/404.html"), Some(baked_404.clone()));

    put(&design, storage, "templates/404.html", "DRAFT-1").await;
    let first = design
        .publish(storage, &tmpl, "alice")
        .await
        .expect("publish");
    assert_eq!(first.by, "alice");
    assert_eq!(
        live(&design, "templates/404.html").as_deref(),
        Some(&b"DRAFT-1"[..])
    );
    let rendered = tmpl.env().get_template("404.html").expect("404").render(());
    assert_eq!(rendered.expect("render"), "DRAFT-1");
    assert!(changes(&design, storage).await.is_empty());
    // From the first publish on, design/ holds the full bundle.
    assert_eq!(
        storage.list("design").await.expect("list").len(),
        first.files
    );
    assert!(
        storage
            .get("design-publish-pending.json")
            .await
            .expect("get")
            .is_none()
    );

    // Deleting from the draft: an extra file is removed live, a baked one
    // reverts to its default.
    design
        .draft_delete(storage, "assets/css/extra.css")
        .await
        .expect("delete extra");
    design
        .draft_delete(storage, "assets/css/style.css")
        .await
        .expect("revert baked");
    put(&design, storage, "templates/404.html", "DRAFT-2").await;
    assert_eq!(
        changes(&design, storage).await,
        [
            ("assets/css/extra.css".to_string(), ChangeKind::Deleted),
            ("templates/404.html".to_string(), ChangeKind::Modified),
        ],
        "a reverted baked file equal to its published copy is no change"
    );
    let second = design
        .publish(storage, &tmpl, "bob")
        .await
        .expect("publish 2");
    assert!(live(&design, "assets/css/extra.css").is_none());
    assert!(
        storage
            .get("design/assets/css/style.css")
            .await
            .expect("get")
            .is_none()
    );
    assert_eq!(
        live(&design, "assets/css/style.css"),
        design.baked("assets/css/style.css")
    );

    let history = design.history(storage).await.expect("history");
    assert_eq!(history, [second.clone(), first.clone()]);

    // Restore goes to the draft, never live.
    design.restore(storage, &first.id).await.expect("restore");
    let read = design.draft_read(storage, "templates/404.html").await;
    assert_eq!(read.expect("read").as_deref(), Some(&b"DRAFT-1"[..]));
    assert_eq!(
        live(&design, "templates/404.html").as_deref(),
        Some(&b"DRAFT-2"[..])
    );
    let restored = changes(&design, storage).await;
    assert!(
        restored.contains(&("assets/css/extra.css".to_string(), ChangeKind::Added)),
        "{restored:?}"
    );

    design.draft_discard(storage).await.expect("discard");
    assert!(changes(&design, storage).await.is_empty());
    let read = design.draft_read(storage, "templates/404.html").await;
    assert_eq!(read.expect("read").as_deref(), Some(&b"DRAFT-2"[..]));

    for missing in ["2001-01-01T00:00:00.000000Z", "latest", "../x"] {
        let err = design.restore(storage, missing).await.expect_err("missing");
        assert!(
            matches!(err, DesignError::NoVersion(_)),
            "{missing}: {err:?}"
        );
    }

    // A second store over the same storage (another process) sees the
    // same draft and history.
    let (other, other_tmpl) = setup();
    other.reload(storage, &other_tmpl).await.expect("reload");
    assert!(changes(&other, storage).await.is_empty());
    assert_eq!(other.history(storage).await.expect("history").len(), 2);
}

#[tokio::test]
async fn draft_publish_history_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&TestStorage::db(&db)).await;
}

#[tokio::test]
async fn draft_publish_history_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn draft_publish_history_over_s3() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&TestStorage::s3(&db)).await;
}
