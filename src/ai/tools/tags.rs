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
use crate::repo::tags::{self as tags_repo, TagInput};

pub struct ListTagsTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ListTagsTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("list_tags")
    }
    fn description(&self) -> &str {
        "List every tag defined in the site."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("list_tags is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        _input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let tags = tags_repo::list_all(&self.db)
            .await
            .context("listing tags")?;
        if tags.is_empty() {
            return Ok(ok_text("No tags defined.".into()));
        }
        let out = tags
            .iter()
            .map(|t| match &t.description {
                Some(d) if !d.is_empty() => format!("{}: {d}", t.name),
                _ => t.name.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ok_text(out))
    }
}

pub struct CreateTagTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for CreateTagTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("create_tag")
    }
    fn description(&self) -> &str {
        "Create a new tag. Tags must exist before they can be assigned to a page via edit_page."
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
        anyhow::bail!("create_tag is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let name = required_str(&args, "name")?;
        if name.trim().is_empty() {
            anyhow::bail!("name is required");
        }
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let saved = tags_repo::create_tag(&self.db, TagInput { name, description })
            .await
            .context("creating tag")?;
        Ok(ok_text(format!(
            "created tag [{}] {}",
            saved.id, saved.name
        )))
    }
}
