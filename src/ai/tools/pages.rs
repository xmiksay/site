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
use crate::repo::pages::{self as pages_repo, PageUpdate, SearchError, UpsertOutcome};
use crate::repo::tags::{self as tags_repo, ResolveError};

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
        Cow::Borrowed("read_page")
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
        anyhow::bail!("read_page is session-scoped; use run_for_session")
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
        let mut out = format!("# {}\n\n", page.path);
        if let Some(s) = &page.summary {
            out.push_str(&format!("Summary: {s}\n"));
        }
        if page.private {
            out.push_str("Private: yes\n");
        }
        out.push_str("\n---\n");
        out.push_str(&page.markdown);
        Ok(ok_text(out))
    }
}

pub struct SearchPagesTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for SearchPagesTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("search_pages")
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
        anyhow::bail!("search_pages is session-scoped; use run_for_session")
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

        let result = match pages_repo::search(
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

        if result.total == 0 {
            return Ok(ok_text("No pages found.".into()));
        }
        let mut out = result
            .pages
            .iter()
            .map(|p| match &p.summary {
                Some(s) if !s.is_empty() => format!("{}: {s}", p.path),
                _ => p.path.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!("\n\n--- total: {} ---", result.total));
        Ok(ok_text(out))
    }
}

pub struct EditPageTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for EditPageTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("edit_page")
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
        anyhow::bail!("edit_page is session-scoped; use run_for_session")
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

        if markdown.is_none() && summary.is_none() && tag_names.is_none() && private.is_none() {
            anyhow::bail!("Nothing to update — provide markdown, summary, tag_names, or private");
        }

        let tag_ids = match &tag_names {
            Some(names) if !names.is_empty() => match tags_repo::resolve_ids(&self.db, names).await
            {
                Ok(ids) => Some(ids),
                Err(ResolveError::Db(e)) => {
                    return Err(anyhow::anyhow!(e).context("resolving tag names"));
                }
                Err(e @ ResolveError::Unknown(_)) => return Err(anyhow::anyhow!(e.to_string())),
            },
            _ => None,
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
        .await
        .context("saving page")?;

        let status = match outcome {
            UpsertOutcome::Created(_) => "created",
            UpsertOutcome::Updated(_) => "updated",
        };
        Ok(ok_text(format!("{status}: {path}")))
    }
}

pub struct DeletePageTool {
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Tool for DeletePageTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("delete_page")
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
        anyhow::bail!("delete_page is session-scoped; use run_for_session")
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
        if deleted {
            Ok(ok_text(format!("deleted: {path}")))
        } else {
            anyhow::bail!("Page not found: {path}")
        }
    }
}
