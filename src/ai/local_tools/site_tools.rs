//! In-process tools that let the assistant manage the site directly, without
//! routing through the MCP HTTP layer. They mirror the most useful tools in
//! `routes::mcp` (page CRUD, tag listing, file listing). Anything more
//! specialised the user can wire in by registering the site's own `/mcp`
//! endpoint as a user MCP server.
//!
//! All CRUD logic lives in `src/repo/*` — these tools are thin adapters that
//! parse JSON args, call repo functions, and format results.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};

use crate::ai::local_tools::{LocalTool, LocalToolCtx};
use crate::ai::mcp_client::ToolDispatchError;
use crate::markdown::MARKDOWN_EXTENSIONS_DOC;
use crate::repo::{
    files::{self as files_repo, NewFile},
    galleries::{self as galleries_repo, GalleryInput as RepoGalleryInput},
    pages::{self as pages_repo, PageUpdate, UpsertOutcome},
    tags::{self as tags_repo, ResolveError, TagInput as RepoTagInput},
};

static EDIT_PAGE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "Create or update a site page by its path. Stores a revision diff automatically when \
         markdown changes.\n\nMarkdown directives available in page content:\n\n{MARKDOWN_EXTENSIONS_DOC}"
    )
});

fn ok_text(text: String) -> Value {
    json!({ "text": text })
}

fn ok_json(v: Value) -> Value {
    let s = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    json!({ "text": s })
}

fn db_err(e: impl std::fmt::Display) -> ToolDispatchError {
    ToolDispatchError::Execution(format!("Database error: {e}"))
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn required_str(args: &Value, key: &str) -> Result<String, ToolDispatchError> {
    arg_str(args, key)
        .map(str::to_string)
        .ok_or_else(|| ToolDispatchError::Execution(format!("{key} is required")))
}

// ----- read_page -----

pub struct ReadPageTool;

#[async_trait]
impl LocalTool for ReadPageTool {
    fn name(&self) -> &str {
        "read_page"
    }
    fn description(&self) -> &str {
        "Read a site page by its path. Returns metadata and full markdown."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let path = required_str(&args, "path")?;
        match pages_repo::find_by_path(&ctx.db, &path).await.map_err(db_err)? {
            Some(p) => {
                let mut out = format!("# {}\n\n", p.path);
                if let Some(s) = &p.summary {
                    out.push_str(&format!("Summary: {s}\n"));
                }
                if p.private {
                    out.push_str("Private: yes\n");
                }
                out.push_str("\n---\n");
                out.push_str(&p.markdown);
                Ok(ok_text(out))
            }
            None => Err(ToolDispatchError::Execution(format!("Page not found: {path}"))),
        }
    }
}

// ----- search_pages -----

pub struct SearchPagesTool;

#[async_trait]
impl LocalTool for SearchPagesTool {
    fn name(&self) -> &str {
        "search_pages"
    }
    fn description(&self) -> &str {
        "Search site pages by path prefix, tag name, and/or fulltext query (q). Path and tag matches outrank markdown content matches."
    }
    fn input_schema(&self) -> Value {
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
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let prefix = arg_str(&args, "prefix").map(str::to_string);
        let tag_name = arg_str(&args, "tag").map(str::to_string);
        let q = arg_str(&args, "q").map(str::to_string);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20).min(100);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

        let result = match pages_repo::search(
            &ctx.db,
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
            Err(pages_repo::SearchError::UnknownTag) => {
                return Ok(ok_text("No pages found.".into()));
            }
            Err(pages_repo::SearchError::Db(e)) => return Err(db_err(e)),
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

// ----- edit_page -----

pub struct EditPageTool;

#[async_trait]
impl LocalTool for EditPageTool {
    fn name(&self) -> &str {
        "edit_page"
    }
    fn description(&self) -> &str {
        EDIT_PAGE_DESCRIPTION.as_str()
    }
    fn input_schema(&self) -> Value {
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
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let path = required_str(&args, "path")?;
        let markdown = arg_str(&args, "markdown").map(String::from);
        let summary = arg_str(&args, "summary").map(String::from);
        let private = args.get("private").and_then(|v| v.as_bool());
        let tag_names: Option<Vec<String>> = args
            .get("tag_names")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect());

        if markdown.is_none() && summary.is_none() && tag_names.is_none() && private.is_none() {
            return Err(ToolDispatchError::Execution(
                "Nothing to update — provide markdown, summary, tag_names, or private".into(),
            ));
        }

        let tag_ids = match &tag_names {
            Some(names) if !names.is_empty() => match tags_repo::resolve_ids(&ctx.db, names).await {
                Ok(ids) => Some(ids),
                Err(ResolveError::Db(e)) => return Err(db_err(e)),
                Err(e @ ResolveError::Unknown(_)) => {
                    return Err(ToolDispatchError::Execution(e.to_string()));
                }
            },
            _ => None,
        };

        let outcome = pages_repo::upsert_by_path(
            &ctx.db,
            ctx.user_id,
            &path,
            PageUpdate {
                markdown,
                summary,
                tag_ids,
                private,
            },
        )
        .await
        .map_err(db_err)?;

        let status = match outcome {
            UpsertOutcome::Created(_) => "created",
            UpsertOutcome::Updated(_) => "updated",
        };
        Ok(ok_text(format!("{status}: {path}")))
    }
}

// ----- list_tags -----

pub struct ListTagsTool;

#[async_trait]
impl LocalTool for ListTagsTool {
    fn name(&self) -> &str {
        "list_tags"
    }
    fn description(&self) -> &str {
        "List every tag defined in the site."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        _args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let tags = tags_repo::list_all(&ctx.db).await.map_err(db_err)?;
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

// ----- create_tag -----

pub struct CreateTagTool;

#[async_trait]
impl LocalTool for CreateTagTool {
    fn name(&self) -> &str {
        "create_tag"
    }
    fn description(&self) -> &str {
        "Create a new tag. Tags must exist before they can be assigned to a page via edit_page."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "description": { "type": "string" }
            },
            "required": ["name"]
        })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let name = required_str(&args, "name")?;
        if name.trim().is_empty() {
            return Err(ToolDispatchError::Execution("name is required".into()));
        }
        let description = arg_str(&args, "description").map(String::from);
        let saved = tags_repo::create_tag(&ctx.db, RepoTagInput { name, description })
            .await
            .map_err(db_err)?;
        Ok(ok_text(format!("created tag [{}] {}", saved.id, saved.name)))
    }
}

// ----- delete_page -----

pub struct DeletePageTool;

#[async_trait]
impl LocalTool for DeletePageTool {
    fn name(&self) -> &str {
        "delete_page"
    }
    fn description(&self) -> &str {
        "Delete a site page by its path."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let path = required_str(&args, "path")?;
        match pages_repo::delete_by_path(&ctx.db, &path).await.map_err(db_err)? {
            true => Ok(ok_text(format!("deleted: {path}"))),
            false => Err(ToolDispatchError::Execution(format!("Page not found: {path}"))),
        }
    }
}

// ----- list_files -----

pub struct ListFilesTool;

#[async_trait]
impl LocalTool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }
    fn description(&self) -> &str {
        "List uploaded files. Optional mime_prefix filter (e.g. 'image/')."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "mime_prefix": { "type": "string" } }
        })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let prefix = arg_str(&args, "mime_prefix").map(String::from);
        let rows = files_repo::list_with_thumbnails(&ctx.db, prefix.as_deref())
            .await
            .map_err(db_err)?;
        if rows.is_empty() {
            return Ok(ok_text("No files.".into()));
        }
        let lines: Vec<String> = rows
            .iter()
            .map(|f| {
                json!({
                    "id": f.model.id,
                    "path": f.model.path,
                    "title": files_repo::title_from_path(&f.model.path),
                    "mimetype": f.model.mimetype,
                    "size_bytes": f.model.size_bytes,
                })
                .to_string()
            })
            .collect();
        Ok(ok_json(json!(lines)))
    }
}

// ----- create_file -----

pub struct CreateFileTool;

#[async_trait]
impl LocalTool for CreateFileTool {
    fn name(&self) -> &str {
        "create_file"
    }
    fn description(&self) -> &str {
        "Upload a file at the given path. Provide either base64-encoded `data_base64` (binary, e.g. \
         images) or raw `data` (text, e.g. PGN/FEN/SVG). Optional `mimetype` (defaults to \
         application/octet-stream) and `description`. The display title is derived from the \
         basename of the path. Returns the new file id — reference it in markdown via \
         `::img{id=ID}` for images or `::gallery{id=ID}` for galleries."
    }
    fn input_schema(&self) -> Value {
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
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let path = required_str(&args, "path")?.trim().to_string();
        if path.is_empty() {
            return Err(ToolDispatchError::Execution("path is required".into()));
        }
        let description = arg_str(&args, "description").map(String::from);
        let mimetype = arg_str(&args, "mimetype")
            .map(String::from)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let data = match (arg_str(&args, "data_base64"), arg_str(&args, "data")) {
            (Some(b64), _) if !b64.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| ToolDispatchError::Execution(format!("Invalid base64: {e}")))?,
            (_, Some(text)) if !text.is_empty() => text.as_bytes().to_vec(),
            _ => {
                return Err(ToolDispatchError::Execution(
                    "either data_base64 or data is required".into(),
                ));
            }
        };
        if data.is_empty() {
            return Err(ToolDispatchError::Execution("decoded data is empty".into()));
        }

        let created = files_repo::create_file(
            &ctx.db,
            ctx.user_id,
            NewFile {
                path,
                description,
                mimetype,
                data,
            },
        )
        .await
        .map_err(db_err)?;

        Ok(ok_json(json!({
            "id": created.model.id,
            "path": created.model.path,
            "title": files_repo::title_from_path(&created.model.path),
            "mimetype": created.model.mimetype,
            "size_bytes": created.model.size_bytes,
            "has_thumbnail": created.has_thumbnail,
            "embed": format!("<image id=\"{}\">", created.model.id),
        })))
    }
}

// ----- list_galleries -----

pub struct ListGalleriesTool;

#[async_trait]
impl LocalTool for ListGalleriesTool {
    fn name(&self) -> &str {
        "list_galleries"
    }
    fn description(&self) -> &str {
        "List every gallery defined in the site."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        _args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let rows = galleries_repo::list_all(&ctx.db).await.map_err(db_err)?;
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

// ----- create_gallery -----

pub struct CreateGalleryTool;

#[async_trait]
impl LocalTool for CreateGalleryTool {
    fn name(&self) -> &str {
        "create_gallery"
    }
    fn description(&self) -> &str {
        "Create a gallery from a list of file IDs. `path` is the unique URL slug (e.g. `holiday-2024`). Reference it in markdown via `::gallery{id=ID}` or `::gallery{path=PATH}`."
    }
    fn input_schema(&self) -> Value {
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
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let path = required_str(&args, "path")?;
        if path.trim().is_empty() {
            return Err(ToolDispatchError::Execution("path is required".into()));
        }
        let title = required_str(&args, "title")?;
        if title.trim().is_empty() {
            return Err(ToolDispatchError::Execution("title is required".into()));
        }
        let description = arg_str(&args, "description").map(String::from);
        let file_ids: Vec<i32> = args
            .get("file_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_i64().map(|n| n as i32)).collect())
            .unwrap_or_default();

        let saved = galleries_repo::create_gallery(
            &ctx.db,
            ctx.user_id,
            RepoGalleryInput {
                path,
                title,
                description,
                file_ids,
            },
        )
        .await
        .map_err(db_err)?;
        Ok(ok_text(format!(
            "created gallery [{}] {} ({} files)",
            saved.id,
            saved.title,
            saved.file_ids.len()
        )))
    }
}

// ----- update_gallery -----

pub struct UpdateGalleryTool;

#[async_trait]
impl LocalTool for UpdateGalleryTool {
    fn name(&self) -> &str {
        "update_gallery"
    }
    fn description(&self) -> &str {
        "Replace a gallery's title, description, and file list. All fields except id are required \
         — pass the existing values for fields you want to keep."
    }
    fn input_schema(&self) -> Value {
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
    async fn call(
        &self,
        ctx: &LocalToolCtx,
        args: Value,
    ) -> Result<Value, ToolDispatchError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| ToolDispatchError::Execution("id is required".into()))?
            as i32;
        let path = required_str(&args, "path")?;
        if path.trim().is_empty() {
            return Err(ToolDispatchError::Execution("path is required".into()));
        }
        let title = required_str(&args, "title")?;
        let description = arg_str(&args, "description").map(String::from);
        let file_ids: Vec<i32> = args
            .get("file_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_i64().map(|n| n as i32)).collect())
            .unwrap_or_default();

        let updated = galleries_repo::update_gallery(
            &ctx.db,
            id,
            RepoGalleryInput {
                path,
                title,
                description,
                file_ids,
            },
        )
        .await
        .map_err(db_err)?;
        match updated {
            Some(g) => Ok(ok_text(format!(
                "updated gallery [{}] {} ({} files)",
                g.id,
                g.title,
                g.file_ids.len()
            ))),
            None => Err(ToolDispatchError::Execution(format!("Gallery not found: {id}"))),
        }
    }
}

pub fn tools() -> Vec<Arc<dyn LocalTool>> {
    vec![
        Arc::new(ReadPageTool) as Arc<dyn LocalTool>,
        Arc::new(SearchPagesTool),
        Arc::new(EditPageTool),
        Arc::new(DeletePageTool),
        Arc::new(ListTagsTool),
        Arc::new(CreateTagTool),
        Arc::new(ListFilesTool),
        Arc::new(CreateFileTool),
        Arc::new(ListGalleriesTool),
        Arc::new(CreateGalleryTool),
        Arc::new(UpdateGalleryTool),
    ]
}
