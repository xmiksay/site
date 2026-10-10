//! Keyed objects (#114): one contract over every backend — `db`
//! (`storage_objects` rows under a random prefix), `fs` and `s3` — plus
//! `storage migrate` copying them between backends.
//!
//! Gated on `DATABASE_URL` like every DB test; the S3 tests fail, not skip,
//! without `TEST_S3_*` (see `common/storage.rs`).

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use bytes::Bytes;
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use site::entity::file_blob;
use site::files::hash_blob;
use site::storage::{Error, migrate};
use storage_fixture::TestStorage;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn keys(objects: &[site::storage::Object]) -> Vec<&str> {
    objects.iter().map(|o| o.key.as_str()).collect()
}

/// The keyed contract every backend honors.
async fn exercise(db: &DatabaseConnection, ts: &TestStorage) {
    let storage = &ts.storage;
    let kind = storage.kind();
    assert!(storage.list("design").await.expect("list").is_empty());

    // `%`/`_` are ordinary key characters, not wildcards; `designer/` is a
    // sibling, not under `design/`.
    let font = "design/assets/my_font%.ttf";
    let base = "design/templates/base.html";
    for (key, body) in [(base, "base"), (font, "font"), ("designer/x", "x")] {
        storage.put(key, Bytes::from(body)).await.expect("put");
    }
    assert_eq!(
        storage.get(base).await.expect("get").as_deref(),
        Some(&b"base"[..])
    );
    assert!(
        storage
            .get("design/missing.css")
            .await
            .expect("get")
            .is_none()
    );

    let listed = storage.list("design").await.expect("list");
    assert_eq!(keys(&listed), [font, base], "{kind}");
    assert_eq!(listed[1].version.size, 4);
    if kind == "db" {
        let etag = listed[1].version.e_tag.as_deref();
        assert_eq!(etag, Some(hash_blob(b"base").as_str()), "etag is sha256");
    }
    assert_eq!(
        keys(&storage.list("design/templates").await.expect("list")),
        [base]
    );

    // Overwrite: new content, new version.
    storage
        .put(base, Bytes::from("rewritten"))
        .await
        .expect("overwrite");
    assert_eq!(
        storage.get(base).await.expect("get").as_deref(),
        Some(&b"rewritten"[..])
    );
    let relisted = storage.list("design/templates").await.expect("list");
    assert_ne!(relisted[0].version, listed[1].version, "{kind}");

    // Keyed objects only: blobs never show up in `list_all`.
    let blob = format!("blob {kind} {}", uuid::Uuid::new_v4());
    let hash = storage.put_blob(blob.as_bytes()).await.expect("put_blob");
    let all = storage.list_all().await.expect("list_all");
    assert_eq!(keys(&all), [font, base, "designer/x"], "{kind}");
    file_blob::Entity::delete_by_id(hash)
        .exec(db)
        .await
        .expect("delete throwaway file_blob");

    storage.delete(base).await.expect("delete");
    storage.delete(base).await.expect("delete is idempotent");
    assert!(storage.get(base).await.expect("get").is_none());
    assert_eq!(keys(&storage.list("design").await.expect("list")), [font]);

    for bad in ["", "design/", "../x", "a//b"] {
        let err = storage.put(bad, Bytes::new()).await.expect_err(bad);
        assert!(matches!(err, Error::InvalidKey(_)), "{bad:?}: {err:?}");
    }
}

#[tokio::test]
async fn db_objects() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::db(&db)).await;
}

#[tokio::test]
async fn fs_objects() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn s3_objects() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::s3(&db)).await;
}

#[tokio::test]
async fn db_scopes_are_isolated() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let (a, b) = (TestStorage::db(&db), TestStorage::db(&db));
    a.storage
        .put("design/x.css", Bytes::from("a"))
        .await
        .expect("put");
    assert!(b.storage.get("design/x.css").await.expect("get").is_none());
    assert!(b.storage.list_all().await.expect("list").is_empty());
    assert_eq!(
        keys(&a.storage.list_all().await.expect("list")),
        ["design/x.css"]
    );
}

/// Copies every keyed object, is a no-op when re-run, and keeps (and
/// reports) a target object with other content.
async fn migrate_objects(source: &TestStorage, target: &TestStorage) {
    let (src, dst) = (&source.storage, &target.storage);
    let keys = ["design/assets/css/style.css", "design/templates/base.html"];
    for key in keys {
        src.put(key, Bytes::from(format!("{key} {}", src.kind())))
            .await
            .expect("source put");
    }

    let report = migrate::migrate(src, dst, &[]).await.expect("migrate");
    assert_eq!(report.objects.copied, keys, "{report:?}");
    assert_eq!(report.problems().count(), 0, "{report:?}");
    for key in keys {
        assert_eq!(
            dst.get(key).await.expect("target get"),
            src.get(key).await.expect("source get")
        );
    }

    let again = migrate::migrate(src, dst, &[]).await.expect("re-run");
    assert_eq!(again.objects.present, keys, "re-run is a no-op: {again:?}");
    assert!(again.objects.copied.is_empty());

    let [_, tampered] = keys;
    dst.put(tampered, Bytes::from("tampered"))
        .await
        .expect("tamper");
    let report = migrate::migrate(src, dst, &[]).await.expect("migrate");
    assert_eq!(report.objects.problems.len(), 1, "{report:?}");
    assert!(report.objects.problems[0].contains("different content"));
    assert_eq!(
        dst.get(tampered).await.expect("get").as_deref(),
        Some(&b"tampered"[..]),
        "never overwritten"
    );
}

#[tokio::test]
async fn migrate_objects_db_to_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    migrate_objects(&TestStorage::db(&db), &TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn migrate_objects_db_to_s3() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    migrate_objects(&TestStorage::db(&db), &TestStorage::s3(&db)).await;
}

#[tokio::test]
async fn migrate_objects_fs_to_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    migrate_objects(&TestStorage::fs(&db), &TestStorage::db(&db)).await;
}
