//! `file_read` — metadata plus, on request, the contents: text for text-ish
//! mimetypes, an image block for raster images (#132, see `super::image`).

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::common::{arg_str, ok_json, parse_args};
use super::image::{image_result, session_sees_images};
use crate::files::is_model_image;
use crate::repo::files as files_repo;
use crate::storage::Storage;

pub struct ReadFileTool {
    pub db: Arc<DatabaseConnection>,
    pub storage: Storage,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("file_read")
    }
    fn description(&self) -> &str {
        "Read file metadata by `id` or `path` (exactly one; e.g. a chat attachment's \
         `uploads/chat/…` path). Set `include_content` to also see the contents: text-ish \
         mimetypes (plain text, JSON, PGN, mermaid, FEN) come back as `content`, raster images \
         (PNG, JPEG, WebP, GIF) as an image you can look at (downscaled to at most 1568 px); \
         other mimetypes get `content: null` with a `content_error` note instead of binary data."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "path": { "type": "string", "description": "The file's path (alternative to id)." },
                "include_content": { "type": "boolean", "description": "Also return the file's contents: text for text-ish mimetypes, the image itself for images (default false)." }
            }
        })
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("file_read is session-scoped; use run_for_session")
    }
    async fn run_for_session(
        &self,
        session: &SessionId,
        _request_id: &str,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let include_content = args
            .get("include_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let id = match (
            args.get("id").and_then(|v| v.as_i64()),
            arg_str(&args, "path"),
        ) {
            (Some(id), None) => id as i32,
            (None, Some(path)) => {
                files_repo::find_by_path(&self.db, path)
                    .await
                    .context("reading file")?
                    .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?
                    .id
            }
            _ => anyhow::bail!("provide exactly one of id or path"),
        };

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
        if include_content && is_model_image(&f.model.mimetype) {
            let Some(data) = self
                .storage
                .get_blob(&f.model.hash)
                .await
                .context("reading file blob")?
            else {
                result["content"] = Value::Null;
                result["content_error"] = json!("blob data missing");
                return Ok(ok_json(result));
            };
            let sees = session_sees_images(&self.db, session).await;
            return Ok(image_result(result, data, f.model.mimetype, sees).await);
        }
        if include_content {
            if files_repo::is_text_content(&f.model.mimetype) {
                match self
                    .storage
                    .get_blob(&f.model.hash)
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
