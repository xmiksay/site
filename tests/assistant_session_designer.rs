//! #118: a chat may enter the `designer` profile — whose design-draft writes
//! need no approval — only before its first prompt. `PATCH
//! /assistant/sessions/{id}` to `designer` succeeds on a fresh session and
//! answers 409 on one with history (its history may carry injected
//! instructions from outside content); creating a Designer chat and
//! switching *out* of `designer` stay allowed. No LLM is called: `SetAgent`
//! only rebinds. Gated on `DATABASE_URL`.

mod common;

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use common::{send, test_db_url};
use entanglement_core::{InMsg, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{assistant_event, assistant_session, llm_model, llm_provider, token, user};

struct Fixture {
    app: Router,
    db: DatabaseConnection,
    cookie: String,
    user_id: i32,
    provider_id: i32,
    model_id: i32,
}

async fn setup(db_url: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage: Default::default(),
    };
    let state = site::state::create_state(&config).await;
    let db = state.db.clone();
    let saved_user = user::ActiveModel {
        username: Set(format!("assistant-designer-{}", uuid::Uuid::new_v4())),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert user");
    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(saved_user.id),
        expires_at: Set(None),
        label: Set(Some("test".to_string())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert session token");
    let provider = llm_provider::ActiveModel {
        label: Set(format!("test-designer-{}", uuid::Uuid::new_v4())),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(Some("http://localhost:11434/v1".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert provider");
    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("model-a".to_string()),
        model: Set("model-a".to_string()),
        is_default: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert model");
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .expect("refresh model catalog");
    let app = site::routes::api::router(state.clone()).with_state(state);
    Fixture {
        app,
        db,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved_user.id,
        provider_id: provider.id,
        model_id: model.id,
    }
}

impl Fixture {
    async fn create(&self, profile: &str) -> i32 {
        let body = json!({ "model_id": self.model_id, "agent_profile": profile });
        let (status, created) = send(
            &self.app,
            "POST",
            "/assistant/sessions",
            &self.cookie,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        created["id"].as_i64().expect("session id") as i32
    }

    async fn set_profile(&self, id: i32, profile: &str) -> (StatusCode, Value) {
        let uri = format!("/assistant/sessions/{id}");
        let body = json!({ "agent_profile": profile });
        send(&self.app, "PATCH", &uri, &self.cookie, Some(body)).await
    }

    async fn engine_id(&self, id: i32) -> SessionId {
        let row = assistant_session::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .expect("load session")
            .expect("session row");
        SessionId::new(row.engine_session_id.expect("engine session id"))
    }

    /// Give the session a persisted prompt, as a first turn would.
    async fn add_history(&self, id: i32) {
        let session = self.engine_id(id).await;
        let record = LogRecord {
            ts: 0,
            session: session.clone(),
            payload: LogPayload::In(InMsg::prompt(session.clone(), "fetch that page")),
        };
        assistant_event::ActiveModel {
            root_session_id: Set(session.0.clone()),
            payload: Set(serde_json::to_value(&record).expect("serialize record")),
            created_at: Set(chrono::Utc::now().into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("insert prompt event");
    }

    async fn cleanup(&self, ids: &[i32]) {
        for id in ids {
            let sid = self.engine_id(*id).await;
            let _ = site::ai::persistence::delete_session_events(&self.db, &sid).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = site::ai::persistence::delete_session_events(&self.db, &sid).await;
        }
        let _ = user::Entity::delete_by_id(self.user_id)
            .exec(&self.db)
            .await;
        let _ = llm_model::Entity::delete_by_id(self.model_id)
            .exec(&self.db)
            .await;
        let _ = llm_provider::Entity::delete_by_id(self.provider_id)
            .exec(&self.db)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn designer_only_for_sessions_without_history() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url).await;

    // A fresh chat may switch into designer, and back out.
    let fresh = fx.create("build").await;
    let (status, body) = fx.set_profile(fresh, "designer").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agent_profile"], "designer");
    let (status, body) = fx.set_profile(fresh, "build").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A chat with history may not: 409, profile unchanged.
    let used = fx.create("build").await;
    fx.add_history(used).await;
    let (status, body) = fx.set_profile(used, "designer").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let error = body["error"].as_str().unwrap_or("");
    assert!(error.contains("start a new Designer chat"), "{body}");
    let row = assistant_session::Entity::find_by_id(used)
        .one(&fx.db)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(row.agent_profile, "build");
    // Other profiles stay switchable.
    let (status, body) = fx.set_profile(used, "researcher").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A Designer chat created as such may leave designer after its first
    // turn, but not come back.
    let designer = fx.create("designer").await;
    fx.add_history(designer).await;
    let (status, body) = fx.set_profile(designer, "designer").await;
    assert_eq!(status, StatusCode::OK, "staying is a no-op: {body}");
    let (status, body) = fx.set_profile(designer, "build").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fx.set_profile(designer, "designer").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    fx.cleanup(&[fresh, used, designer]).await;
}
