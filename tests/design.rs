//! Design overrides in storage (#110): the reload contract of
//! `DesignStore::apply` over fs and S3 (and a dead S3), and `design push`.
//! The HTTP routes are in `tests/design_api.rs`.
//!
//! Gated on `DATABASE_URL` like every DB test; the S3 test fails, not skips,
//! without `TEST_S3_*` (see `common/storage.rs`).

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use std::sync::Arc;

use axum::http::StatusCode;
use sea_orm::{Database, DatabaseConnection};
use site::design::DesignStore;
use site::design::stored::{Change, DesignError};
use site::templates::Templates;
use storage_fixture::TestStorage;

fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

async fn test_db() -> Option<DatabaseConnection> {
    let url = test_db_url()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn put(path: &str, body: &str) -> Option<Change> {
    Some(Change::Put {
        path: path.into(),
        bytes: body.as_bytes().to_vec().into(),
    })
}

/// The `apply` contract every object backend honors.
async fn exercise_apply(ts: &TestStorage) {
    let design = Arc::new(DesignStore::new(None));
    let tmpl = Templates::new(design.clone());
    let storage = &ts.storage;
    let baked_css = design.load("assets/css/style.css").expect("baked css");

    // Admin save: written to storage and live at once.
    design
        .apply(storage, &tmpl, put("assets/css/style.css", "body{}"))
        .await
        .expect("save");
    assert_eq!(
        design.load("assets/css/style.css").as_deref(),
        Some(&b"body{}"[..])
    );
    let stored = storage
        .get("design/assets/css/style.css")
        .await
        .expect("get");
    assert_eq!(stored.as_deref(), Some(&b"body{}"[..]));

    // A broken template is rejected: nothing written, nothing swapped.
    let err = design
        .apply(storage, &tmpl, put("templates/404.html", "{% if %}"))
        .await
        .expect_err("broken template");
    assert!(matches!(err, DesignError::Invalid(_)), "{err:?}");
    assert!(
        storage
            .get("design/templates/404.html")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        design.last_reload().expect("status").ok,
        "a rejected save keeps the status"
    );

    // Edited straight in the bucket, then Reload.
    storage
        .put("design/templates/404.html", "BUCKET-404".into())
        .await
        .expect("external put");
    let status = design.apply(storage, &tmpl, None).await.expect("reload");
    assert_eq!(status.files, 2);
    let rendered = tmpl
        .env()
        .get_template("404.html")
        .expect("404")
        .render(())
        .expect("render");
    assert_eq!(rendered, "BUCKET-404");

    // A broken file in the bucket fails the reload and keeps the design.
    storage
        .put("design/templates/404.html", "{% endif %}".into())
        .await
        .expect("external broken put");
    design
        .apply(storage, &tmpl, None)
        .await
        .expect_err("broken reload");
    let status = design.last_reload().expect("status");
    assert!(!status.ok && status.error.as_deref().unwrap_or("").contains("404.html"));
    assert_eq!(
        design.load("templates/404.html").as_deref(),
        Some(&b"BUCKET-404"[..])
    );

    // Delete the override (and fix the bucket): back to baked.
    storage
        .delete("design/templates/404.html")
        .await
        .expect("rm");
    design
        .apply(
            storage,
            &tmpl,
            Some(Change::Delete {
                path: "assets/css/style.css".into(),
            }),
        )
        .await
        .expect("delete");
    assert_eq!(design.load("assets/css/style.css"), Some(baked_css));
    assert!(design.stored_paths().is_empty());
    let err = design
        .apply(
            storage,
            &tmpl,
            Some(Change::Delete {
                path: "assets/css/style.css".into(),
            }),
        )
        .await
        .expect_err("second delete");
    assert!(matches!(err, DesignError::NoOverride(_)), "{err:?}");
}

#[tokio::test]
async fn apply_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_apply(&TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn apply_over_s3() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_apply(&TestStorage::s3(&db)).await;
}

#[tokio::test]
async fn push_uploads_bundle_files_once_and_refuses_broken_templates() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let src = std::env::temp_dir().join(format!("design-push-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(src.join("templates")).expect("mkdir");
    std::fs::create_dir_all(src.join("assets/css")).expect("mkdir");
    std::fs::write(src.join("templates/404.html"), "pushed").expect("write");
    std::fs::write(src.join("assets/css/style.css"), "a{}").expect("write");
    std::fs::write(src.join("README.md"), "not a bundle file").expect("write");
    std::fs::write(src.join(".hidden"), "x").expect("write");

    let report = site::design::push::push(&ts.storage, &src)
        .await
        .expect("push");
    assert_eq!(
        report.uploaded,
        ["assets/css/style.css", "templates/404.html"]
    );
    assert_eq!(report.skipped, ["README.md"]);
    let again = site::design::push::push(&ts.storage, &src)
        .await
        .expect("re-push");
    assert!(
        again.uploaded.is_empty() && again.unchanged.len() == 2,
        "{again:?}"
    );

    std::fs::write(src.join("templates/404.html"), "{% if %}").expect("write");
    std::fs::write(src.join("assets/css/style.css"), "changed{}").expect("write");
    let err = site::design::push::push(&ts.storage, &src)
        .await
        .expect_err("broken");
    assert!(err.to_string().contains("templates/404.html"), "{err}");
    let css = ts
        .storage
        .get("design/assets/css/style.css")
        .await
        .expect("get");
    assert_eq!(
        css.as_deref(),
        Some(&b"a{}"[..]),
        "nothing uploaded on a broken push"
    );

    let _ = std::fs::remove_dir_all(&src);
}

#[tokio::test]
async fn reload_against_unreachable_s3_is_503_and_recorded() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let dead = site::storage::Storage::s3(&storage_fixture::dead_s3_config(), db).expect("dead");
    let design = Arc::new(DesignStore::new(None));
    let tmpl = Templates::new(design.clone());
    let err = design
        .apply(&dead, &tmpl, None)
        .await
        .expect_err("dead reload");
    let api: site::routes::api::error::ApiError = err.into();
    let resp = axum::response::IntoResponse::into_response(api);
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!design.last_reload().expect("status").ok);
}
