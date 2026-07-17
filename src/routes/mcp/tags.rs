//! MCP tools for the `tags` family: `list_tags`, `read_tag`, `create_tag`,
//! `update_tag`, `delete_tag`.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::repo::{
    format,
    tags::{self as tags_repo, TagInput as RepoTagInput, TagSaveError, TagUpdate as RepoTagUpdate},
};
use crate::routes::broadcast;
use crate::state::AppState;

use super::rpc::{JsonRpcResponse, json_result, parse_args, tool_error, tool_result};

#[derive(Deserialize)]
struct TagArgs {
    name: String,
}

#[derive(Deserialize)]
struct TagInputArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct UpdateTagArgs {
    name: String,
    #[serde(default)]
    new_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

pub(super) async fn tool_list_tags(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    match tags_repo::list_all(&state.db).await {
        Ok(tags) => tool_result(id, format::format_tags(&tags)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_read_tag(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: TagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match tags_repo::find_by_name(&state.db, &args.name).await {
        Ok(Some(t)) => json_result(
            id,
            json!({ "id": t.id, "name": t.name, "description": t.description }),
        ),
        Ok(None) => tool_error(id, &format!("Tag not found: {}", args.name)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_create_tag(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: TagInputArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match tags_repo::create_tag(
        &state.db,
        RepoTagInput {
            name: args.name,
            description: args.description,
        },
    )
    .await
    {
        Ok(t) => {
            broadcast::tag_created(&state.ws_hub, &t);
            tool_result(id, format!("created tag [{}] {}", t.id, t.name))
        }
        Err(e @ TagSaveError::EmptyName) => tool_error(id, &e.to_string()),
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

pub(super) async fn tool_update_tag(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: UpdateTagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let name = args.name.clone();
    match tags_repo::update_tag_by_name(
        &state.db,
        &name,
        RepoTagUpdate {
            new_name: args.new_name,
            description: args.description,
        },
    )
    .await
    {
        Ok(Some(t)) => {
            broadcast::tag_updated(&state.ws_hub, &t);
            tool_result(id, format!("updated tag [{}] {}", t.id, t.name))
        }
        Ok(None) => tool_error(id, &format!("Tag not found: {name}")),
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

pub(super) async fn tool_delete_tag(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: TagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match tags_repo::delete_by_name(&state.db, &args.name).await {
        Ok(Some(tag_id)) => {
            broadcast::tag_deleted(&state.ws_hub, tag_id);
            tool_result(id, format!("deleted tag {}", args.name))
        }
        Ok(None) => tool_error(id, &format!("Tag not found: {}", args.name)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}
