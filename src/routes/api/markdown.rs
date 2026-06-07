use axum::{Json, Router, extract::State, routing::post};
use serde::{Deserialize, Serialize};

use crate::markdown;
use crate::routes::api::error::ApiResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/render", post(render))
}

#[derive(Deserialize)]
pub struct RenderInput {
    pub markdown: String,
}

#[derive(Serialize)]
pub struct RenderOutput {
    pub html: String,
}

pub async fn render(
    State(state): State<AppState>,
    Json(input): Json<RenderInput>,
) -> ApiResult<Json<RenderOutput>> {
    let env = state.tmpl.env();
    let html = markdown::render(&input.markdown, &state.db, &env, true).await;
    Ok(Json(RenderOutput { html }))
}
