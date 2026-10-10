//! The design-draft tools (#118), one implementation behind both edges: the
//! MCP server (`routes::mcp::design`) and the AI assistant
//! (`ai::tools::design`). They only ever touch the shared draft — there is
//! deliberately no publish tool: publishing is a human action in the admin.
//! Writes broadcast `design.draft_changed` exactly like the admin API.

pub mod specs;

use base64::Engine;
use bytes::Bytes;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::DesignStore;
use super::check::render_check;
use super::stored::{DesignError, Files, check_path, status_error};
use crate::mcp_args;
use crate::routes::broadcast::{self, DraftChange};
use crate::routes::ws::WsHub;
use crate::storage::Storage;
use crate::templates::contract;

/// Fonts and images are the largest files; templates are tiny.
pub const MAX_FILE_SIZE: usize = 20 * 1024 * 1024;

/// What a design tool needs from the app.
pub struct Ctx<'a> {
    pub db: &'a DatabaseConnection,
    pub storage: &'a Storage,
    pub design: &'a DesignStore,
    pub hub: &'a WsHub,
}

pub enum Output {
    Text(String),
    Json(Value),
}

/// A failure as the caller may see it: storage and DB detail is logged,
/// never returned (external MCP clients see these).
#[derive(Debug, PartialEq, Eq)]
pub struct ToolError(pub String);

impl From<DesignError> for ToolError {
    fn from(err: DesignError) -> Self {
        if matches!(err, DesignError::Storage(_)) {
            tracing::error!("design tool: {err}");
        }
        Self(status_error(&err))
    }
}

/// One file of a draft view, as `design_list` and `GET /api/design/draft`
/// show it.
#[derive(Debug, Serialize)]
pub struct DesignFile {
    pub path: String,
    pub baked: bool,
    /// Differs from the baked default (or has none).
    pub overridden: bool,
    pub size: u64,
}

/// Every file of `view` (a full draft or published view), sorted by path.
pub fn file_entries(design: &DesignStore, view: &Files) -> Vec<DesignFile> {
    view.iter()
        .map(|(path, bytes)| {
            let baked = design.baked(path);
            DesignFile {
                baked: baked.is_some(),
                overridden: baked.as_deref() != Some(bytes.as_ref()),
                size: bytes.len() as u64,
                path: path.clone(),
            }
        })
        .collect()
}

/// Which version of a design file to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Draft,
    Published,
    Baked,
}

impl Source {
    pub fn parse(source: Option<&str>) -> Result<Self, String> {
        match source {
            None | Some("draft") => Ok(Self::Draft),
            Some("published") => Ok(Self::Published),
            Some("baked") => Ok(Self::Baked),
            Some(other) => Err(format!(
                "source must be draft, published or baked, not {other:?}"
            )),
        }
    }
}

/// `path` as `source` holds it; `None` when it has no such file.
pub async fn read_source(
    design: &DesignStore,
    storage: &Storage,
    path: &str,
    source: Source,
) -> Result<Option<Bytes>, DesignError> {
    check_path(path)?;
    Ok(match source {
        Source::Draft => design.draft_read(storage, path).await?,
        Source::Published => design.published_view().remove(path),
        Source::Baked => design.baked(path).map(Bytes::from),
    })
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    data_base64: Option<String>,
}

#[derive(Deserialize)]
struct ContractArgs {
    #[serde(default)]
    schema: bool,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    mcp_args::parse_value(args).map_err(ToolError)
}

/// Run design tool `name` (one of [`specs::TOOLS`]).
pub async fn call(ctx: &Ctx<'_>, name: &str, args: Value) -> Result<Output, ToolError> {
    match name {
        specs::LIST => list(ctx, parse(args)?).await,
        specs::READ => read(ctx, parse(args)?).await,
        specs::WRITE => write(ctx, parse(args)?).await,
        specs::DELETE => delete(ctx, parse::<PathArgs>(args)?.path).await,
        specs::CHANGES => {
            let draft = ctx.design.draft(ctx.storage).await?;
            Ok(Output::Json(json!({
                "initialized": draft.initialized,
                "changes": draft.changes,
            })))
        }
        specs::CONTRACT => Ok(Output::Text(if parse::<ContractArgs>(args)?.schema {
            contract::json_schema_text()
        } else {
            contract::markdown()
        })),
        specs::RENDER_CHECK => check(ctx).await,
        other => Err(ToolError(format!("unknown design tool {other:?}"))),
    }
}

async fn list(ctx: &Ctx<'_>, args: ListArgs) -> Result<Output, ToolError> {
    let draft = ctx.design.draft(ctx.storage).await?;
    let prefix = args.prefix.unwrap_or_default();
    let files: Vec<DesignFile> = file_entries(ctx.design, &ctx.design.with_baked(&draft.files))
        .into_iter()
        .filter(|f| f.path.starts_with(&prefix))
        .collect();
    Ok(Output::Json(json!({
        "initialized": draft.initialized,
        "files": files,
        "changes": draft.changes,
    })))
}

async fn read(ctx: &Ctx<'_>, args: PathArgs) -> Result<Output, ToolError> {
    let source = Source::parse(args.source.as_deref()).map_err(ToolError)?;
    let bytes = read_source(ctx.design, ctx.storage, &args.path, source)
        .await?
        .ok_or_else(|| ToolError(format!("{} not found", args.path)))?;
    let mimetype = mime_guess::from_path(&args.path).first_or_octet_stream();
    let (key, data) = match std::str::from_utf8(&bytes) {
        Ok(text) => ("data", text.to_string()),
        Err(_) => (
            "data_base64",
            base64::engine::general_purpose::STANDARD.encode(&bytes),
        ),
    };
    Ok(Output::Json(json!({
        "path": args.path,
        "mimetype": mimetype.to_string(),
        "size": bytes.len(),
        key: data,
    })))
}

/// The bytes of a write: exactly one of `data` / `data_base64`.
fn write_bytes(args: &WriteArgs) -> Result<Bytes, ToolError> {
    let bytes = match (&args.data, &args.data_base64) {
        (Some(text), None) => Bytes::from(text.clone()),
        (None, Some(b64)) => {
            // Refuse before decoding: base64 of MAX_FILE_SIZE bytes is longer.
            let b64 = b64.trim();
            if b64.len() > MAX_FILE_SIZE.div_ceil(3) * 4 {
                return Err(too_large(b64.len() / 4 * 3));
            }
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map(Bytes::from)
                .map_err(|e| ToolError(format!("invalid data_base64: {e}")))?
        }
        _ => {
            return Err(ToolError(
                "provide exactly one of data or data_base64".into(),
            ));
        }
    };
    if bytes.len() > MAX_FILE_SIZE {
        return Err(too_large(bytes.len()));
    }
    Ok(bytes)
}

fn too_large(size: usize) -> ToolError {
    ToolError(format!(
        "file too large: about {size} bytes (max {MAX_FILE_SIZE})"
    ))
}

async fn write(ctx: &Ctx<'_>, args: WriteArgs) -> Result<Output, ToolError> {
    check_path(&args.path)?;
    let bytes = write_bytes(&args)?;
    let size = bytes.len();
    ctx.design.draft_put(ctx.storage, &args.path, bytes).await?;
    broadcast::design_draft_changed(ctx.hub, &DraftChange::Put { path: &args.path });
    Ok(Output::Json(json!({
        "path": args.path,
        "size": size,
        "note": "written to the draft (not live); run design_render_check",
    })))
}

async fn delete(ctx: &Ctx<'_>, path: String) -> Result<Output, ToolError> {
    ctx.design.draft_delete(ctx.storage, &path).await?;
    broadcast::design_draft_changed(ctx.hub, &DraftChange::Delete { path: &path });
    let outcome = if ctx.design.baked(&path).is_some() {
        "reverted to the baked default"
    } else {
        "removed from the draft"
    };
    Ok(Output::Text(format!("{path}: {outcome}")))
}

async fn check(ctx: &Ctx<'_>) -> Result<Output, ToolError> {
    let draft = ctx.design.draft(ctx.storage).await?;
    let view = ctx.design.with_baked(&draft.files);
    let report = render_check(ctx.db, ctx.storage, &view).await?;
    Ok(Output::Json(json!({
        "ok": report.is_ok(),
        "compile_errors": report.compile_errors,
        "render_errors": report.render_errors,
        "cases": report.cases,
    })))
}

#[cfg(test)]
mod tests;
