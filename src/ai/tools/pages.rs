//! Page CRUD tools — port of `local_tools::site_tools::{ReadPageTool,
//! SearchPagesTool, EditPageTool, DeletePageTool}` to `Tool`.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use anyhow::Context;
use async_trait::async_trait;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::common::{arg_str, ok_text, parse_args, required_str};
use crate::ai::engine::user_id_from_session;
use crate::markdown::MARKDOWN_EXTENSIONS_DOC;
use crate::repo::format;
use crate::repo::pages::{self as pages_repo, PageSaveError, PageUpdate, UpsertOutcome};
use crate::repo::pages_search::{self as pages_search_repo, SearchError};
use crate::repo::tags as tags_repo;
use crate::routes::broadcast;
use crate::routes::ws::WsHub;

static EDIT_PAGE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "Create or update a site page by its path. Stores a revision diff automatically when \
         markdown changes.\n\nMarkdown directives available in page content:\n\n{MARKDOWN_EXTENSIONS_DOC}"
    )
});

pub struct ReadPageTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for ReadPageTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("page_read")
    }
    fn description(&self) -> &str {
        "Read a site page by its path. Returns metadata and full markdown."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("page_read is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let path = required_str(&args, "path")?;
        let page = pages_repo::find_by_path(&self.db, &path)
            .await
            .context("reading page")?
            .ok_or_else(|| anyhow::anyhow!("Page not found: {path}"))?;
        let tag_names = tags_repo::resolve_names(&self.db, &page.tag_ids).await;
        Ok(ok_text(format::format_page(&page, &tag_names)))
    }
}

pub struct SearchPagesTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for SearchPagesTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("page_search")
    }
    fn description(&self) -> &str {
        "Search site pages by path prefix, tag name, and/or fulltext query (q). Path and tag \
         matches outrank markdown content matches."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prefix": { "type": "string" },
                "tag": { "type": "string" },
                "q": { "type": "string" },
                "limit": { "type": "integer" },
                "offset": { "type": "integer" }
            }
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("page_search is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let prefix = arg_str(&args, "prefix").map(str::to_string);
        let tag_name = arg_str(&args, "tag").map(str::to_string);
        let q = arg_str(&args, "q").map(str::to_string);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(100);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

        let result = match pages_search_repo::search(
            &self.db,
            prefix.as_deref(),
            tag_name.as_deref(),
            q.as_deref(),
            true,
            limit,
            offset,
        )
        .await
        {
            Ok(r) => r,
            Err(SearchError::UnknownTag) => return Ok(ok_text("No pages found.".into())),
            Err(SearchError::Db(e)) => return Err(anyhow::anyhow!(e).context("searching pages")),
        };

        Ok(ok_text(format::format_search_results(
            &result, limit, offset,
        )))
    }
}

pub struct EditPageTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for EditPageTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("page_edit")
    }
    fn description(&self) -> &str {
        EDIT_PAGE_DESCRIPTION.as_str()
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "markdown": { "type": "string" },
                "summary": { "type": "string" },
                "tag_names": { "type": "array", "items": { "type": "string" } },
                "private": { "type": "boolean" }
            },
            "required": ["path"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("page_edit is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let user_id = user_id_from_session(session)?;
        let args = parse_args(input)?;
        let path = required_str(&args, "path")?;
        let markdown = arg_str(&args, "markdown").map(String::from);
        let summary = arg_str(&args, "summary").map(String::from);
        let private = args.get("private").and_then(|v| v.as_bool());
        let tag_names: Option<Vec<String>> =
            args.get("tag_names").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });

        pages_repo::validate_page_edit_fields(&markdown, &summary, &tag_names, &private)
            .map_err(anyhow::Error::msg)?;

        let (tag_ids, skipped) = match &tag_names {
            Some(names) if !names.is_empty() => match tags_repo::resolve_ids(&self.db, names).await
            {
                Ok(r) => (Some(r.ids), r.missing),
                Err(e) => return Err(anyhow::anyhow!(e).context("resolving tag names")),
            },
            _ => (None, Vec::new()),
        };

        let outcome = pages_repo::upsert_by_path(
            &self.db,
            user_id,
            &path,
            PageUpdate {
                markdown,
                summary,
                tag_ids,
                private,
            },
        )
        .await;

        let status = match outcome {
            Ok(UpsertOutcome::Created(page)) => {
                broadcast::page_created(&self.ws_hub, &page);
                "created"
            }
            Ok(UpsertOutcome::Updated(page)) => {
                broadcast::page_updated(&self.ws_hub, &page);
                "updated"
            }
            Err(e @ PageSaveError::EmptyPath) => anyhow::bail!("{e}"),
            Err(e) => return Err(anyhow::anyhow!(e).context("saving page")),
        };
        Ok(ok_text(format!(
            "{status}: {path}{}",
            tags_repo::skipped_note(&skipped)
        )))
    }
}

pub struct DeletePageTool {
    pub db: Arc<DatabaseConnection>,
    pub ws_hub: Arc<WsHub>,
}

#[async_trait]
impl Tool for DeletePageTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("page_delete")
    }
    fn description(&self) -> &str {
        "Delete a site page by its path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("page_delete is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let path = required_str(&args, "path")?;
        let deleted = pages_repo::delete_by_path(&self.db, &path)
            .await
            .context("deleting page")?;
        if let Some(page_id) = deleted {
            broadcast::page_deleted(&self.ws_hub, page_id);
            Ok(ok_text(format!("deleted: {path}")))
        } else {
            anyhow::bail!("Page not found: {path}")
        }
    }
}
