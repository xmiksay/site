//! Chat attachments (#132): `POST /api/assistant/sessions/{id}/attachments`
//! stores a normal chat's upload as a site file under `uploads/chat/YYYY-MM/`
//! (a collision gets a `-2` suffix) and a Designer chat's in the design draft
//! (`assets/img/`, `assets/fonts/`; other types 422); over 10 MB is 413. At
//! the tool level, `file_read` / `design_read` on an image answer an image
//! block — or a text note for a model flagged `supports_images = false`.
//! Over fs storage, so the shared draft is this test's own. No LLM is called.
//! Gated on `DATABASE_URL`.

#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use entanglement_core::SessionId;
use entanglement_provider::{ContentPart, ImageSource, ToolCall};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{assistant_session, file, file_thumbnail, llm_model, llm_provider, token, user};
use site::state::AppState;
use storage_fixture::TestStorage;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    app: Router,
    cookie: String,
    user_id: i32,
    provider_id: i32,
    model_id: i32,
    // Removes the fs storage root on drop.
    _storage: TestStorage,
}

async fn setup(db_url: &str) -> Fixture {
    let db = Database::connect(db_url).await.expect("connect");
    let ts = TestStorage::fs(&db);
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage: ts.fs_config(),
    };
    let state = site::state::create_state(&config).await;
    let db = &state.db;
    let saved_user = user::ActiveModel {
        username: Set(format!("attachments-{}", uuid::Uuid::new_v4())),
        password_hash: Set("unused".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert user");
    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(saved_user.id),
        expires_at: Set(None),
        label: Set(Some("test".into())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert session token");
    let provider = llm_provider::ActiveModel {
        label: Set(format!("test-attachments-{}", uuid::Uuid::new_v4())),
        kind: Set("ollama".into()),
        api_key: Set(None),
        base_url: Set(Some("http://localhost:11434/v1".into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert provider");
    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("model-a".into()),
        model: Set("model-a".into()),
        is_default: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert model");
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .expect("refresh model catalog");
    let app = Router::new()
        .nest("/api", site::routes::api::router(state.clone()))
        .with_state(state.clone());
    Fixture {
        state,
        app,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved_user.id,
        provider_id: provider.id,
        model_id: model.id,
        _storage: ts,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("encode png");
    buf
}

impl Fixture {
    async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn create(&self, profile: &str) -> assistant_session::Model {
        let body = json!({ "model_id": self.model_id, "agent_profile": profile });
        let req = Request::builder()
            .method("POST")
            .uri("/api/assistant/sessions")
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        let (status, created) = self.send(req).await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["id"].as_i64().expect("session id") as i32;
        assistant_session::Entity::find_by_id(id)
            .one(&self.state.db)
            .await
            .expect("query")
            .expect("session row")
    }

    async fn attach(&self, session: i32, filename: &str, data: &[u8]) -> (StatusCode, Value) {
        let boundary = format!("b-{}", uuid::Uuid::new_v4());
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/assistant/sessions/{session}/attachments"))
            .header("cookie", &self.cookie)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .expect("request");
        self.send(req).await
    }

    async fn tool(
        &self,
        session: &assistant_session::Model,
        name: &str,
        input: Value,
    ) -> Vec<ContentPart> {
        let registry = site::ai::tools::registry(
            std::sync::Arc::new(self.state.db.clone()),
            self.state.storage.clone(),
            self.state.design.clone(),
            self.state.ws_hub.clone(),
            None,
        );
        let sid = SessionId::new(session.engine_session_id.clone().expect("engine session"));
        registry
            .execute(&ToolCall::new("call-1", name, input.to_string()), &sid)
            .await
    }

    async fn cleanup(self, paths: &[String]) {
        let db = &self.state.db;
        // Thumbnails do not cascade; the blob is content-addressed and may be
        // shared with other rows (the PNG is deterministic), so it stays.
        let rows = file::Entity::find()
            .filter(file::Column::Path.is_in(paths.iter().cloned()))
            .all(db)
            .await
            .expect("query files");
        file_thumbnail::Entity::delete_many()
            .filter(file_thumbnail::Column::FileId.is_in(rows.iter().map(|f| f.id)))
            .exec(db)
            .await
            .expect("delete thumbnails");
        file::Entity::delete_many()
            .filter(file::Column::Path.is_in(paths.iter().cloned()))
            .exec(db)
            .await
            .expect("delete files");
        user::Entity::delete_by_id(self.user_id)
            .exec(db)
            .await
            .expect("delete user");
        llm_provider::Entity::delete_by_id(self.provider_id)
            .exec(db)
            .await
            .expect("delete provider");
    }
}

fn image_width(parts: &[ContentPart]) -> u32 {
    let Some(ContentPart::Image {
        source: ImageSource::Base64 { data, .. },
    }) = parts.get(1)
    else {
        panic!("expected [meta, image], got {parts:?}");
    };
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64");
    image::load_from_memory(&bytes).expect("decodable").width()
}

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

#[tokio::test]
async fn concurrent_same_name_attachments_get_distinct_paths() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&url).await;
    let chat = fx.create("build").await;
    let name = format!("race-{}.txt", uuid::Uuid::new_v4());
    let (a, b) = tokio::join!(
        fx.attach(chat.id, &name, b"one"),
        fx.attach(chat.id, &name, b"two")
    );
    assert_eq!(
        (a.0, b.0),
        (StatusCode::CREATED, StatusCode::CREATED),
        "{a:?} {b:?}"
    );
    let files = [
        a.1["path"].as_str().expect("path"),
        b.1["path"].as_str().expect("path"),
    ];
    assert_ne!(files[0], files[1]);

    let designer = fx.create("designer").await;
    let (a, b) = tokio::join!(
        fx.attach(designer.id, "race.png", b"one"),
        fx.attach(designer.id, "race.png", b"two")
    );
    assert_eq!(
        (a.0, b.0),
        (StatusCode::CREATED, StatusCode::CREATED),
        "{a:?} {b:?}"
    );
    let mut assets = [a.1["path"].clone(), b.1["path"].clone()];
    assets.sort_by_key(|p| p.to_string());
    assert_eq!(
        assets,
        [json!("assets/img/race-2.png"), json!("assets/img/race.png")]
    );

    let paths = files.map(String::from);
    fx.cleanup(&paths).await;
}
