//! MCP tools for the `pages` family: `page_read`, `page_edit`,
//! `page_search`, `page_delete`.

use serde::Deserialize;
use serde_json::Value;

use crate::repo::{
    format,
    pages::{self as pages_repo, PageSaveError, PageUpdate, UpsertOutcome},
    pages_search::{self as pages_search_repo, SearchError},
    tags::{self as tags_repo},
};
use crate::routes::broadcast;
use crate::state::AppState;

use super::rpc::{JsonRpcResponse, parse_args, tool_error, tool_result};

#[derive(Deserialize)]
struct ReadPageArgs {
    path: String,
}

#[derive(Deserialize)]
struct EditPageArgs {
    path: String,
    markdown: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    tag_names: Option<Vec<String>>,
    #[serde(default)]
    private: Option<bool>,
}

#[derive(Deserialize)]
struct SearchPagesArgs {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct DeletePageArgs {
    path: String,
}

pub(super) async fn tool_page_read(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: ReadPageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };

    match pages_repo::find_by_path(&state.db, &args.path).await {
        Ok(Some(p)) => {
            let tag_names = tags_repo::resolve_names(&state.db, &p.tag_ids).await;
            tool_result(id, format::format_page(&p, &tag_names))
        }
        Ok(None) => tool_error(id, &format!("Page not found: {}", args.path)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_page_edit(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: EditPageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if let Err(msg) = pages_repo::validate_page_edit_fields(
        &args.markdown,
        &args.summary,
        &args.tag_names,
        &args.private,
    ) {
        return tool_error(id, msg);
    }

    let (tag_ids, skipped) = match &args.tag_names {
        Some(names) if !names.is_empty() => match tags_repo::resolve_ids(&state.db, names).await {
            Ok(r) => (Some(r.ids), r.missing),
            Err(e) => return tool_error(id, &format!("Database error: {e}")),
        },
        _ => (None, Vec::new()),
    };

    let outcome = pages_repo::upsert_by_path(
        &state.db,
        user_id,
        &args.path,
        PageUpdate {
            markdown: args.markdown,
            summary: args.summary,
            tag_ids,
            private: args.private,
        },
    )
    .await;

    match outcome {
        Ok(UpsertOutcome::Created(page)) => {
            broadcast::page_created(&state.ws_hub, &page);
            tool_result(
                id,
                format!(
                    "created: {}{}",
                    args.path,
                    tags_repo::skipped_note(&skipped)
                ),
            )
        }
        Ok(UpsertOutcome::Updated(page)) => {
            broadcast::page_updated(&state.ws_hub, &page);
            tool_result(
                id,
                format!(
                    "updated: {}{}",
                    args.path,
                    tags_repo::skipped_note(&skipped)
                ),
            )
        }
        Err(e @ PageSaveError::EmptyPath) => tool_error(id, &e.to_string()),
        Err(e) => tool_error(id, &format!("Save failed: {e}")),
    }
}

pub(super) async fn tool_page_search(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: SearchPagesArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let limit = args.limit.unwrap_or(20).min(100);
    let offset = args.offset.unwrap_or(0);

    let result = match pages_search_repo::search(
        &state.db,
        args.prefix.as_deref(),
        args.tag.as_deref(),
        args.q.as_deref(),
        true,
        limit,
        offset,
    )
    .await
    {
        Ok(r) => r,
        Err(SearchError::UnknownTag) => {
            return tool_result(id, "No pages found.".into());
        }
        Err(SearchError::Db(e)) => {
            return tool_error(id, &format!("Database error: {e}"));
        }
    };

    tool_result(id, format::format_search_results(&result, limit, offset))
}

pub(super) async fn tool_page_delete(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: DeletePageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match pages_repo::delete_by_path(&state.db, &args.path).await {
        Ok(Some(page_id)) => {
            broadcast::page_deleted(&state.ws_hub, page_id);
            tool_result(id, format!("deleted: {}", args.path))
        }
        Ok(None) => tool_error(id, &format!("Page not found: {}", args.path)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}
