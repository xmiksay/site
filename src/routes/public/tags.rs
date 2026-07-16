use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use sea_orm::EntityTrait;

use crate::entity::tag;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/{id}", get(by_tag))
}

fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub async fn by_tag(State(state): State<AppState>, Path(tag_id): Path<i32>) -> Response {
    match tag::Entity::find_by_id(tag_id).one(&state.db).await {
        Ok(Some(t)) => {
            Redirect::to(&format!("/search?tag={}", encode_query(&t.name))).into_response()
        }
        _ => Redirect::to("/search").into_response(),
    }
}
