//! Chat attachments (#132): `POST /api/assistant/sessions/{id}/attachments`
//! stores a normal chat's upload as a site file under `uploads/chat/YYYY-MM/`
//! (a collision gets a `-2` suffix) and a Designer chat's in the design draft
//! (`assets/img/`, `assets/fonts/`; other types 422); over 10 MB is 413, an
//! unknown or foreign session 404. At the tool level, `file_read` /
//! `design_read` on an image answer an image block — or a text note for a
//! model flagged `supports_images = false`. Concurrent same-name uploads are
//! in `assistant_attachments_race.rs`; the harness is
//! `common/attachments_app.rs`. Gated on `DATABASE_URL`.

#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

#[path = "common/attachments_app.rs"]
mod attachments_app;

use attachments_app::{image_width, png, setup};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{EntityTrait, Set};
use serde_json::{Value, json};
use site::entity::{file, llm_model};
use tower::ServiceExt;

#[tokio::test]
async fn normal_chat_attachment_is_a_site_file_readable_as_an_image() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&url).await;
    let session = fx.create("build").await;
    let name = format!("Shot {}.PNG", uuid::Uuid::new_v4());
    let data = png(2000, 50);

    let (status, first) = fx.attach(session.id, &name, &data).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let path = first["path"].as_str().expect("path").to_string();
    let month = chrono::Utc::now()
        .format("uploads/chat/%Y-%m/shot-")
        .to_string();
    assert!(path.starts_with(&month) && path.ends_with(".png"), "{path}");
    assert_eq!(first["target"], "file");
    assert_eq!(first["mimetype"], "image/png");
    let row = file::Entity::find_by_id(first["file_id"].as_i64().expect("id") as i32)
        .one(&fx.state.db)
        .await
        .expect("query")
        .expect("file row");
    assert_eq!(row.path, path);

    // The transcript links an attachment by path.
    let req = Request::builder()
        .uri(format!("/api/files/by-path/{path}?thumbnail=true"))
        .header("cookie", &fx.cookie)
        .body(Body::empty())
        .expect("request");
    let resp = fx.app.clone().oneshot(req).await.expect("response");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers()["location"].to_str().expect("location");
    assert_eq!(location, format!("/files/{}/nahled", row.hash));

    let (status, second) = fx.attach(session.id, &name, &data).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let second_path = second["path"].as_str().expect("path").to_string();
    assert_eq!(second_path, path.replace(".png", "-2.png"));

    let parts = fx
        .tool(
            &session,
            "file_read",
            json!({ "path": path, "include_content": true }),
        )
        .await;
    assert_eq!(image_width(&parts), 1568);
    let meta: Value = serde_json::from_str(parts[0].as_text().expect("meta")).expect("json");
    assert_eq!(meta["path"], path.as_str());

    // Metadata only unless asked.
    let parts = fx
        .tool(&session, "file_read", json!({ "path": path }))
        .await;
    assert_eq!(parts.len(), 1);

    llm_model::Entity::update(llm_model::ActiveModel {
        id: Set(fx.model_id),
        supports_images: Set(false),
        ..Default::default()
    })
    .exec(&fx.state.db)
    .await
    .expect("flag model blind");
    let parts = fx
        .tool(
            &session,
            "file_read",
            json!({ "path": path, "include_content": true }),
        )
        .await;
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert!(
        parts[0]
            .as_text()
            .expect("text")
            .contains("cannot view images")
    );

    fx.cleanup(&[path, second_path]).await;
}

#[tokio::test]
async fn designer_chat_attachment_lands_in_the_draft() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&url).await;
    let session = fx.create("designer").await;

    let (status, font) = fx.attach(session.id, "Inter Var.woff2", b"wOF2-font").await;
    assert_eq!(status, StatusCode::CREATED, "{font}");
    assert_eq!(font["path"], "assets/fonts/inter-var.woff2");
    assert_eq!(font["target"], "design");
    assert!(font.get("file_id").is_none());
    let stored = fx
        .state
        .design
        .draft_read(&fx.state.storage, "assets/fonts/inter-var.woff2")
        .await
        .expect("draft read");
    assert_eq!(stored.as_deref(), Some(&b"wOF2-font"[..]));

    let (status, img) = fx.attach(session.id, "logo.png", &png(1600, 1600)).await;
    assert_eq!(status, StatusCode::CREATED, "{img}");
    assert_eq!(img["path"], "assets/img/logo.png");
    let parts = fx
        .tool(
            &session,
            "design_read",
            json!({ "path": "assets/img/logo.png" }),
        )
        .await;
    assert_eq!(image_width(&parts), 1568);

    let (status, err) = fx.attach(session.id, "notes.txt", b"hello").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");

    fx.cleanup(&[]).await;
}

#[tokio::test]
async fn attachments_over_ten_megabytes_are_refused() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&url).await;
    let session = fx.create("build").await;
    let big = vec![0u8; 10 * 1024 * 1024 + 1];
    let (status, err) = fx.attach(session.id, "big.bin", &big).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
    assert_eq!(err["code"], "too_large");

    let (status, _) = fx.attach(session.id + 1_000_000, "a.png", b"x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Another user's chat is as unknown as a missing one.
    let other = setup(&url).await;
    let foreign = other.create("build").await;
    let (status, _) = fx.attach(foreign.id, "a.png", b"x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    other.cleanup(&[]).await;
    fx.cleanup(&[]).await;
}
