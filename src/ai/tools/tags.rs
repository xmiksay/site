//! Tag tools — port of `local_tools::site_tools::{ListTagsTool, CreateTagTool}`.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::common::{ok_text, parse_args, required_str};
use crate::repo::format;
use crate::repo::tags::{self as tags_repo, TagInput, TagSaveError};
use crate::routes::broadcast;
use crate::routes::ws::WsHub;

pub struct ListTagsTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ListTagsTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("tag_list")
    }
    fn description(&self) -> &str {
        "List every tag defined in the site."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("tag_list is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        _input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let tags = tags_repo::list_all(&self.db)
            .await
            .context("listing tags")?;
        Ok(ok_text(format::format_tags(&tags)))
    }
}

pub struct CreateTagTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for CreateTagTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("tag_create")
    }
    fn description(&self) -> &str {
        "Create a new tag. Tags must exist before they can be assigned to a page via page_edit."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "description": { "type": "string" }
            },
            "required": ["name"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("tag_create is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let name = required_str(&args, "name")?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let saved = match tags_repo::create_tag(&self.db, TagInput { name, description }).await {
            Ok(t) => t,
            Err(e @ TagSaveError::EmptyName) => anyhow::bail!("{e}"),
            Err(e) => return Err(anyhow::anyhow!(e).context("creating tag")),
        };
        broadcast::tag_created(&self.ws_hub, &saved);
        Ok(ok_text(format!(
            "created tag [{}] {}",
            saved.id, saved.name
        )))
    }
}
