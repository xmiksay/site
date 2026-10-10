//! The published design in storage (#110, #114): the reload contract of
//! `DesignStore::reload` over db, fs and S3 (and a dead S3), and `design
//! push` over db and fs. The draft/publish/history flow is in
//! `tests/design_publish.rs`, the HTTP routes in `tests/design_api.rs`.
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

/// The `reload` contract every backend honors: objects edited straight in
/// the bucket go live on reload; a broken one fails it and keeps the design.
async fn exercise_reload(ts: &TestStorage) {
    let design = Arc::new(DesignStore::new(None));
    let tmpl = Templates::new(design.clone());
    let storage = &ts.storage;
    let baked_404 = design.load("templates/404.html").expect("baked 404");

    let status = design.reload(storage, &tmpl).await.expect("empty reload");
    assert!(status.ok && status.files == 0, "{status:?}");

    storage
        .put("design/templates/404.html", "BUCKET-404".into())
        .await
        .expect("external put");
    storage
        .put("design/assets/css/style.css", "body{}".into())
        .await
        .expect("external put");
    // Outside the bundle roots: ignored.
    storage
        .put("design/notes.txt", "x".into())
        .await
        .expect("external put");
    let status = design.reload(storage, &tmpl).await.expect("reload");
    assert_eq!(status.files, 2);
    let rendered = tmpl
        .env()
        .get_template("404.html")
        .expect("404")
        .render(())
        .expect("render");
    assert_eq!(rendered, "BUCKET-404");

    storage
        .put("design/templates/404.html", "{% endif %}".into())
        .await
        .expect("external broken put");
    design
        .reload(storage, &tmpl)
        .await
        .expect_err("broken reload");
    let status = design.last_reload().expect("status");
    assert!(!status.ok && status.error.as_deref().unwrap_or("").contains("404.html"));
    assert_eq!(
        design.load("templates/404.html").as_deref(),
        Some(&b"BUCKET-404"[..])
    );

    for key in ["design/templates/404.html", "design/assets/css/style.css"] {
        storage.delete(key).await.expect("rm");
    }
    design.reload(storage, &tmpl).await.expect("reload");
    assert_eq!(design.load("templates/404.html"), Some(baked_404));
}

#[tokio::test]
async fn reload_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_reload(&TestStorage::db(&db)).await;
}

#[tokio::test]
async fn reload_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_reload(&TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn reload_over_s3() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_reload(&TestStorage::s3(&db)).await;
}

async fn exercise_push(ts: &TestStorage) {
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
async fn push_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_push(&TestStorage::db(&db)).await;
}

#[tokio::test]
async fn push_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise_push(&TestStorage::fs(&db)).await;
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
    let err = design.reload(&dead, &tmpl).await.expect_err("dead reload");
    let api: site::routes::api::error::ApiError = err.into();
    let resp = axum::response::IntoResponse::into_response(api);
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!design.last_reload().expect("status").ok);
}
