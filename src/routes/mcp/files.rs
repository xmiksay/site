//! MCP tools for the `files` family: `file_list`, `file_create`,
//! `file_read`, `file_update`, `file_delete`.

use serde::Deserialize;
use serde_json::{Value, json};

use base64::Engine;

use crate::repo::files::{self as files_repo, FileMetaUpdate, FileSaveError, NewFile};
use crate::routes::broadcast;
use crate::state::AppState;

use super::rpc::{JsonRpcResponse, json_result, parse_args, tool_error, tool_result};

#[derive(Deserialize)]
struct ListFilesArgs {
    #[serde(default)]
    mime_prefix: Option<String>,
}

#[derive(Deserialize)]
struct FileIdArgs {
    id: i32,
}

#[derive(Deserialize)]
struct ReadFileArgs {
    id: i32,
    #[serde(default)]
    include_content: Option<bool>,
}

#[derive(Deserialize)]
struct CreateFileArgs {
    path: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    mimetype: Option<String>,
    #[serde(default)]
    data_base64: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct UpdateFileArgs {
    id: i32,
    path: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    mimetype: Option<String>,
    #[serde(default)]
    data_base64: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

pub(super) async fn tool_file_list(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: ListFilesArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::list_with_thumbnails(&state.db, args.mime_prefix.as_deref()).await {
        Ok(rows) if rows.is_empty() => tool_result(id, "No files.".into()),
        Ok(rows) => {
            let lines: Vec<String> = rows
                .iter()
                .map(|f| {
                    format!(
                        "[{}] {} ({}, {} bytes)",
                        f.model.id, f.model.path, f.model.mimetype, f.model.size_bytes
                    )
                })
                .collect();
            tool_result(id, lines.join("\n"))
        }
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_file_create(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: CreateFileArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let data = match (args.data_base64, args.data) {
        (Some(b64), _) if !b64.is_empty() => {
            match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(d) => d,
                Err(e) => return tool_error(id, &format!("Invalid base64: {e}")),
            }
        }
        (_, Some(text)) if !text.is_empty() => text.into_bytes(),
        _ => return tool_error(id, "either data_base64 or data is required"),
    };

    let mimetype = args
        .mimetype
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| files_repo::infer_mimetype(&args.path));

    match files_repo::create_file(
        &state.db,
        user_id,
        NewFile {
            path: args.path,
            description: args.description,
            mimetype,
            data,
        },
    )
    .await
    {
        Ok(created) => {
            let summary =
                broadcast::file_created(&state.ws_hub, &created.model, created.has_thumbnail);
            let embed = files_repo::embed_hint(
                &created.model.path,
                &created.model.mimetype,
                created.model.id,
            );
            json_result(
                id,
                json!({
                    "id": created.model.id,
                    "path": created.model.path,
                    "title": summary.title,
                    "mimetype": created.model.mimetype,
                    "size_bytes": created.model.size_bytes,
                    "has_thumbnail": created.has_thumbnail,
                    "embed": embed,
                }),
            )
        }
        Err(e @ (FileSaveError::EmptyPath | FileSaveError::EmptyData)) => {
            tool_error(id, &e.to_string())
        }
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

pub(super) async fn tool_file_read(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: ReadFileArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::find_with_thumbnail(&state.db, args.id).await {
        Ok(Some(f)) => {
            let title = files_repo::title_from_path(&f.model.path);
            let mut result = json!({
                "id": f.model.id,
                "path": f.model.path,
                "title": title,
                "description": f.model.description,
                "mimetype": f.model.mimetype,
                "size_bytes": f.model.size_bytes,
                "has_thumbnail": f.has_thumbnail,
                "created_at": f.model.created_at.to_string(),
            });
            if args.include_content.unwrap_or(false) {
                if files_repo::is_text_content(&f.model.mimetype) {
                    match crate::files::read_blob(&state.db, &f.model.hash).await {
                        Ok(Some(data)) => {
                            result["content"] = json!(String::from_utf8_lossy(&data));
                        }
                        Ok(None) => {
                            result["content"] = Value::Null;
                            result["content_error"] = json!("blob data missing");
                        }
                        Err(e) => {
                            return tool_error(id, &format!("Database error: {e}"));
                        }
                    }
                } else {
                    result["content"] = Value::Null;
                    result["content_error"] = json!("not a text mimetype");
                }
            }
            json_result(id, result)
        }
        Ok(None) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

pub(super) async fn tool_file_update(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: UpdateFileArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let data = match (args.data_base64, args.data) {
        (Some(b64), _) if !b64.is_empty() => {
            match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(d) => Some(d),
                Err(e) => return tool_error(id, &format!("Invalid base64: {e}")),
            }
        }
        (_, Some(text)) if !text.is_empty() => Some(text.into_bytes()),
        _ => None,
    };

    match files_repo::update_metadata(
        &state.db,
        args.id,
        FileMetaUpdate {
            path: args.path,
            description: args.description,
            mimetype: args.mimetype,
            data,
        },
    )
    .await
    {
        Ok(Some(f)) => {
            broadcast::file_updated(&state.ws_hub, &f.model, f.has_thumbnail);
            tool_result(
                id,
                format!("updated file [{}] {}", f.model.id, f.model.path),
            )
        }
        Ok(None) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e @ (FileSaveError::EmptyPath | FileSaveError::EmptyData)) => {
            tool_error(id, &e.to_string())
        }
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

pub(super) async fn tool_file_delete(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: FileIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::delete_by_id(&state.db, args.id).await {
        Ok(true) => {
            broadcast::file_deleted(&state.ws_hub, args.id);
            tool_result(id, format!("deleted file {}", args.id))
        }
        Ok(false) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}
