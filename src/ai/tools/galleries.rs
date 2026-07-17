//! Gallery tools — port of `local_tools::site_tools::{ListGalleriesTool,
//! CreateGalleryTool, UpdateGalleryTool}`.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::common::{arg_str, ok_text, parse_args, required_str};
use crate::ai::engine::user_id_from_session;
use crate::repo::galleries::{self as galleries_repo, GalleryInput, GallerySaveError};
use crate::routes::broadcast;
use crate::routes::ws::WsHub;

pub struct ListGalleriesTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ListGalleriesTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("list_galleries")
    }
    fn description(&self) -> &str {
        "List every gallery defined in the site."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("list_galleries is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        _input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let rows = galleries_repo::list_all(&self.db)
            .await
            .context("listing galleries")?;
        if rows.is_empty() {
            return Ok(ok_text("No galleries.".into()));
        }
        let out = rows
            .iter()
            .map(|g| format!("[{}] {} ({} files)", g.id, g.title, g.file_ids.len()))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ok_text(out))
    }
}

fn parse_file_ids(args: &Value) -> Vec<i32> {
    args.get("file_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_i64().map(|n| n as i32))
                .collect()
        })
        .unwrap_or_default()
}

pub struct CreateGalleryTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for CreateGalleryTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("create_gallery")
    }
    fn description(&self) -> &str {
        "Create a gallery from a list of file IDs. `path` is the unique URL slug (e.g. \
         `holiday-2024`). Reference it in markdown via `<gallery id=\"ID\">` or `<gallery \
         path=\"PATH\">`."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "file_ids": { "type": "array", "items": { "type": "integer" } }
            },
            "required": ["path", "title"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("create_gallery is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let user_id = user_id_from_session(session)?;
        let args = parse_args(input)?;
        let path = required_str(&args, "path")?;
        let title = required_str(&args, "title")?;
        let description = arg_str(&args, "description").map(String::from);
        let file_ids = parse_file_ids(&args);

        let saved = match galleries_repo::create_gallery(
            &self.db,
            user_id,
            GalleryInput {
                path,
                title,
                description,
                file_ids,
            },
        )
        .await
        {
            Ok(g) => g,
            Err(e @ (GallerySaveError::EmptyPath | GallerySaveError::EmptyTitle)) => {
                anyhow::bail!("{e}")
            }
            Err(e) => return Err(anyhow::anyhow!(e).context("creating gallery")),
        };
        broadcast::gallery_created(&self.ws_hub, &saved);
        Ok(ok_text(format!(
            "created gallery [{}] {} ({} files)",
            saved.id,
            saved.title,
            saved.file_ids.len()
        )))
    }
}

pub struct UpdateGalleryTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for UpdateGalleryTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("update_gallery")
    }
    fn description(&self) -> &str {
        "Replace a gallery's title, description, and file list. All fields except id are \
         required — pass the existing values for fields you want to keep."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "path": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "file_ids": { "type": "array", "items": { "type": "integer" } }
            },
            "required": ["id", "path", "title"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("update_gallery is session-scoped; use run_for_session")
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
        let path = required_str(&args, "path")?;
        let title = required_str(&args, "title")?;
        let description = arg_str(&args, "description").map(String::from);
        let file_ids = parse_file_ids(&args);

        let updated = match galleries_repo::update_gallery(
            &self.db,
            id,
            GalleryInput {
                path,
                title,
                description,
                file_ids,
            },
        )
        .await
        {
            Ok(updated) => updated,
            Err(e @ GallerySaveError::EmptyPath) => anyhow::bail!("{e}"),
            Err(e) => return Err(anyhow::anyhow!(e).context("updating gallery")),
        };
        match updated {
            Some(g) => {
                broadcast::gallery_updated(&self.ws_hub, &g);
                Ok(ok_text(format!(
                    "updated gallery [{}] {} ({} files)",
                    g.id,
                    g.title,
                    g.file_ids.len()
                )))
            }
            None => anyhow::bail!("Gallery not found: {id}"),
        }
    }
}
