//! The chat-attachments test harness (#132), shared by
//! `assistant_attachments.rs` and `assistant_attachments_race.rs`: a full
//! `AppState` over fs storage (so the shared draft is the test's own), a
//! logged-in throwaway user with an ollama provider/model row (never
//! called), and helpers to create sessions, upload attachments and run a
//! built-in tool. The including test declares `storage_fixture`
//! (`common/storage.rs`).

use crate::storage_fixture::TestStorage;
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
use tower::ServiceExt;

pub struct Fixture {
    pub state: AppState,
    pub app: Router,
    pub cookie: String,
    pub user_id: i32,
    pub provider_id: i32,
    pub model_id: i32,
    // Removes the fs storage root on drop.
    _storage: TestStorage,
}

pub async fn setup(db_url: &str) -> Fixture {
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

pub fn png(width: u32, height: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("encode png");
    buf
}

impl Fixture {
    pub async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub async fn create(&self, profile: &str) -> assistant_session::Model {
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

    pub async fn attach(&self, session: i32, filename: &str, data: &[u8]) -> (StatusCode, Value) {
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

    pub async fn tool(
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

    pub async fn cleanup(self, paths: &[String]) {
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

pub fn image_width(parts: &[ContentPart]) -> u32 {
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
