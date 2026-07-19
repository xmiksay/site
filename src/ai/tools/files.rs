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
use crate::repo::files::{self as files_repo, FileSaveError, NewFile};
use crate::routes::broadcast;
use crate::routes::ws::WsHub;

pub struct ListFilesTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("list_files")
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
        anyhow::bail!("list_files is session-scoped; use run_for_session")
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
        Cow::Borrowed("create_file")
    }
    fn description(&self) -> &str {
        "Upload a file at the given path. Provide either base64-encoded `data_base64` (binary, \
         e.g. images) or raw `data` (text, e.g. PGN/FEN/SVG). Optional `mimetype` (defaults to \
         application/octet-stream) and `description`. The display title is derived from the \
         basename of the path. Returns the new file id plus an `embed` hint for the matching \
         markdown directive: `<image id=\"ID\">` for images, `<pgn id=\"ID\">` for `.pgn`, \
         `<mermaid id=\"ID\">` for `.mmd`, `<fen id=\"ID\">` for `.fen`, `<json id=\"ID\" \
         query=\"...\">` for `.json`, `<file id=\"ID\">` otherwise (or `<gallery id=\"ID\">` \
         to group several files)."
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
        anyhow::bail!("create_file is session-scoped; use run_for_session")
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
            .unwrap_or_else(|| "application/octet-stream".to_string());

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
