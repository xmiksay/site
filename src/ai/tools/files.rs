//! File tools — port of `local_tools::site_tools::{ListFilesTool, CreateFileTool}`.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use base64::Engine;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::common::{arg_str, ok_json, ok_text, parse_args, required_str};
use crate::ai::engine::user_id_from_session;
use crate::repo::files::{self as files_repo, FileMetaUpdate, FileSaveError, NewFile};
use crate::routes::broadcast;
use crate::routes::ws::WsHub;

pub struct ListFilesTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_list")
    }
    fn description(&self) -> &str {
        "List uploaded files. Optional mime_prefix filter (e.g. 'image/')."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "mime_prefix": { "type": "string" } }
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_list is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let prefix = arg_str(&args, "mime_prefix").map(String::from);
        let rows = files_repo::list_with_thumbnails(&self.db, prefix.as_deref())
            .await
            .context("listing files")?;
        if rows.is_empty() {
            return Ok(ok_text("No files.".into()));
        }
        let lines: Vec<Value> = rows
            .iter()
            .map(|f| {
                json!({
                    "id": f.model.id,
                    "path": f.model.path,
                    "title": files_repo::title_from_path(&f.model.path),
                    "mimetype": f.model.mimetype,
                    "size_bytes": f.model.size_bytes,
                })
            })
            .collect();
        Ok(ok_json(json!(lines)))
    }
}

pub struct CreateFileTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_create")
    }
    fn description(&self) -> &str {
        "Upload a file at the given path. Provide either base64-encoded `data_base64` (binary, \
         e.g. images) or raw `data` (text, e.g. PGN/FEN/SVG). Optional `mimetype` (inferred \
         from the path's extension when omitted) and `description`. The display title is \
         derived from the basename of the path. Returns the new file id plus an `embed` hint \
         for the matching markdown directive: `<image id=\"ID\">` for images, `<pgn id=\"ID\">` \
         for `.pgn`, `<mermaid id=\"ID\">` for `.mmd`, `<fen id=\"ID\">` for `.fen`, `<json \
         id=\"ID\" query=\"...\">` for `.json`, `<file id=\"ID\">` otherwise (or `<gallery \
         id=\"ID\">` to group several files)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Filename or path used as the file's unique identifier." },
                "description": { "type": "string" },
                "mimetype": { "type": "string" },
                "data_base64": { "type": "string", "description": "Base64-encoded binary contents." },
                "data": { "type": "string", "description": "Raw text contents (alternative to data_base64)." }
            },
            "required": ["path"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_create is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let user_id = user_id_from_session(session)?;
        let args = parse_args(input)?;
        let path = required_str(&args, "path")?.trim().to_string();
        let description = arg_str(&args, "description").map(String::from);
        let mimetype = arg_str(&args, "mimetype")
            .map(String::from)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| files_repo::infer_mimetype(&path));

        let data = match (arg_str(&args, "data_base64"), arg_str(&args, "data")) {
            (Some(b64), _) if !b64.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .context("invalid base64")?,
            (_, Some(text)) if !text.is_empty() => text.as_bytes().to_vec(),
            _ => anyhow::bail!("either data_base64 or data is required"),
        };

        let created = match files_repo::create_file(
            &self.db,
            user_id,
            NewFile {
                path,
                description,
                mimetype,
                data,
            },
        )
        .await
        {
            Ok(created) => created,
            Err(e @ (FileSaveError::EmptyPath | FileSaveError::EmptyData)) => {
                anyhow::bail!("{e}")
            }
            Err(e) => return Err(anyhow::anyhow!(e).context("creating file")),
        };
        let summary = broadcast::file_created(&self.ws_hub, &created.model, created.has_thumbnail);
        let embed = files_repo::embed_hint(
            &created.model.path,
            &created.model.mimetype,
            created.model.id,
        );

        Ok(ok_json(json!({
            "id": created.model.id,
            "path": created.model.path,
            "title": summary.title,
            "mimetype": created.model.mimetype,
            "size_bytes": created.model.size_bytes,
            "has_thumbnail": created.has_thumbnail,
            "embed": embed,
        })))
    }
}

pub struct ReadFileTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_read")
    }
    fn description(&self) -> &str {
        "Read file metadata by ID. Set `include_content` to also return the file's contents — \
         only populated for text-ish mimetypes (plain text, JSON, PGN, mermaid, FEN); other \
         mimetypes get `content: null` with a `content_error` note instead of binary data."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "include_content": { "type": "boolean", "description": "Also return the file's text contents when the mimetype is text-ish (default false)." }
            },
            "required": ["id"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_read is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("id is required"))? as i32;
        let include_content = args
            .get("include_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let f = files_repo::find_with_thumbnail(&self.db, id)
            .await
            .context("reading file")?
            .ok_or_else(|| anyhow::anyhow!("File not found: {id}"))?;

        let mut result = json!({
            "id": f.model.id,
            "path": f.model.path,
            "title": files_repo::title_from_path(&f.model.path),
            "description": f.model.description,
            "mimetype": f.model.mimetype,
            "size_bytes": f.model.size_bytes,
            "has_thumbnail": f.has_thumbnail,
            "created_at": f.model.created_at.to_string(),
        });
        if include_content {
            if files_repo::is_text_content(&f.model.mimetype) {
                match crate::files::read_blob(self.db.as_ref(), &f.model.hash)
                    .await
                    .context("reading file blob")?
                {
                    Some(data) => result["content"] = json!(String::from_utf8_lossy(&data)),
                    None => {
                        result["content"] = Value::Null;
                        result["content_error"] = json!("blob data missing");
                    }
                }
            } else {
                result["content"] = Value::Null;
                result["content_error"] = json!("not a text mimetype");
            }
        }

        Ok(ok_json(result))
    }
}

pub struct UpdateFileTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for UpdateFileTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_update")
    }
    fn description(&self) -> &str {
        "Update a file's metadata (path, description) and/or replace its contents. Provide \
         either base64-encoded `data_base64` (binary, e.g. images) or raw `data` (text, e.g. \
         PGN/FEN/SVG) to replace the stored bytes, with an optional `mimetype` to match; a \
         thumbnail is regenerated automatically when content changes and the file is an image. \
         The display title is always derived from the path basename."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "path": { "type": "string" },
                "description": { "type": "string" },
                "mimetype": { "type": "string", "description": "New mimetype, e.g. image/png. Only applied when provided." },
                "data_base64": { "type": "string", "description": "Base64-encoded binary contents to replace the file with." },
                "data": { "type": "string", "description": "Raw text contents to replace the file with (alternative to data_base64)." }
            },
            "required": ["id", "path"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_update is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("id is required"))? as i32;
        let path = required_str(&args, "path")?.trim().to_string();
        let description = arg_str(&args, "description").map(String::from);
        let mimetype = arg_str(&args, "mimetype").map(String::from);

        let data = match (arg_str(&args, "data_base64"), arg_str(&args, "data")) {
            (Some(b64), _) if !b64.is_empty() => Some(
                base64::engine::general_purpose::STANDARD
                    .decode(b64.as_bytes())
                    .context("invalid base64")?,
            ),
            (_, Some(text)) if !text.is_empty() => Some(text.as_bytes().to_vec()),
            _ => None,
        };

        let updated = match files_repo::update_metadata(
            &self.db,
            id,
            FileMetaUpdate {
                path,
                description,
                mimetype,
                data,
            },
        )
        .await
        {
            Ok(Some(f)) => f,
            Ok(None) => anyhow::bail!("File not found: {id}"),
            Err(e @ (FileSaveError::EmptyPath | FileSaveError::EmptyData)) => {
                anyhow::bail!("{e}")
            }
            Err(e) => return Err(anyhow::anyhow!(e).context("updating file")),
        };
        broadcast::file_updated(&self.ws_hub, &updated.model, updated.has_thumbnail);

        Ok(ok_json(json!({
            "id": updated.model.id,
            "path": updated.model.path,
            "mimetype": updated.model.mimetype,
            "size_bytes": updated.model.size_bytes,
            "has_thumbnail": updated.has_thumbnail,
        })))
    }
}

pub struct DeleteFileTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_delete")
    }
    fn description(&self) -> &str {
        "Delete a file by ID."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "required": ["id"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_delete is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("id is required"))? as i32;
        let deleted = files_repo::delete_by_id(&self.db, id)
            .await
            .context("deleting file")?;
        if deleted {
            broadcast::file_deleted(&self.ws_hub, id);
            Ok(ok_text(format!("deleted file {id}")))
        } else {
            anyhow::bail!("File not found: {id}")
        }
    }
}
