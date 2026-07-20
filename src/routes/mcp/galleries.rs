//! MCP tools for the `galleries` family: `gallery_list`, `gallery_read`,
//! `gallery_create`, `gallery_update`, `gallery_delete`.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::repo::galleries::{
    self as galleries_repo, GalleryInput as RepoGalleryInput, GallerySaveError,
};
use crate::routes::broadcast;
use crate::state::AppState;

use super::rpc::{JsonRpcResponse, json_result, parse_args, tool_error, tool_result};

#[derive(Deserialize)]
struct GalleryIdArgs {
    id: i32,
}

#[derive(Deserialize)]
struct CreateGalleryArgs {
    path: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    file_ids: Vec<i32>,
}

#[derive(Deserialize)]
struct UpdateGalleryArgs {
    id: i32,
    path: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    file_ids: Vec<i32>,
}

pub(super) async fn tool_gallery_list(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    match galleries_repo::list_all(&state.db).await {
        Ok(rows) if rows.is_empty() => tool_result(id, "No galleries.".into()),
        Ok(rows) => {
            let lines: Vec<String> = rows
                .iter()
                .map(|g| format!("[{}] {} ({} files)", g.id, g.title, g.file_ids.len()))
                .collect();
            tool_result(id, lines.join("\n"))
        }
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_gallery_read(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: GalleryIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match galleries_repo::find_by_id(&state.db, args.id).await {
        Ok(Some(g)) => json_result(
            id,
            json!({
                "id": g.id,
                "title": g.title,
                "description": g.description,
                "file_ids": g.file_ids,
                "created_at": g.created_at.to_string(),
            }),
        ),
        Ok(None) => tool_error(id, &format!("Gallery not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_gallery_create(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: CreateGalleryArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match galleries_repo::create_gallery(
        &state.db,
        user_id,
        RepoGalleryInput {
            path: args.path,
            title: args.title,
            description: args.description,
            file_ids: args.file_ids,
        },
    )
    .await
    {
        Ok(g) => {
            broadcast::gallery_created(&state.ws_hub, &g);
            tool_result(id, format!("created gallery [{}] {}", g.id, g.title))
        }
        Err(e @ (GallerySaveError::EmptyTitle | GallerySaveError::EmptyPath)) => {
            tool_error(id, &e.to_string())
        }
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

pub(super) async fn tool_gallery_update(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: UpdateGalleryArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let gallery_id = args.id;
    match galleries_repo::update_gallery(
        &state.db,
        gallery_id,
        RepoGalleryInput {
            path: args.path,
            title: args.title,
            description: args.description,
            file_ids: args.file_ids,
        },
    )
    .await
    {
        Ok(Some(g)) => {
            broadcast::gallery_updated(&state.ws_hub, &g);
            tool_result(id, format!("updated gallery [{}] {}", g.id, g.title))
        }
        Ok(None) => tool_error(id, &format!("Gallery not found: {gallery_id}")),
        Err(e @ GallerySaveError::EmptyPath) => tool_error(id, &e.to_string()),
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

pub(super) async fn tool_gallery_delete(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: GalleryIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match galleries_repo::delete_by_id(&state.db, args.id).await {
        Ok(true) => {
            broadcast::gallery_deleted(&state.ws_hub, args.id);
            tool_result(id, format!("deleted gallery {}", args.id))
        }
        Ok(false) => tool_error(id, &format!("Gallery not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}
