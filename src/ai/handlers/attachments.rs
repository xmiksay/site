//! `POST /api/assistant/sessions/{id}/attachments` (#132): store one file
//! attached to a chat message and answer where it went. The message itself
//! only names that path — the model opens the file on request with
//! `file_read` / `design_read`, so nothing lands in the prompt unasked.
//! A Designer chat's attachment becomes a design-draft asset; any other
//! chat's a regular site file under `uploads/chat/YYYY-MM/`.

use std::collections::HashSet;

use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::extract::{Extension, Multipart, Path, State};
use axum::http::StatusCode;
use bytes::Bytes;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;

use super::sessions::load_owned;
use crate::ai::engine::DESIGNER_PROFILE;
use crate::design::tools::file_mimetype;
use crate::entity::file;
use crate::repo::files::{self as files_repo, NewFile};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::broadcast::{self, DraftChange};
use crate::state::AppState;

pub const MAX_ATTACHMENT_SIZE: usize = 10 * 1024 * 1024;
/// The route's body limit: one maximal file plus its multipart framing.
pub const BODY_LIMIT: usize = MAX_ATTACHMENT_SIZE + 64 * 1024;
/// `name.png` … `name-999.png`; past that the folder is someone's dump.
const MAX_CANDIDATES: u32 = 999;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico"];
const FONT_EXTENSIONS: &[&str] = &["woff", "woff2", "ttf", "otf", "eot"];

#[derive(Debug, Serialize)]
pub struct Attachment {
    pub path: String,
    pub mimetype: String,
    pub size: usize,
    /// `file` (a site file, read with `file_read`) or `design` (a design
    /// draft asset, read with `design_read`).
    pub target: &'static str,
    /// The site file's id; absent for a design asset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i32>,
}

struct Upload {
    name: String,
    mimetype: Option<String>,
    data: Vec<u8>,
}

pub async fn upload(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<Attachment>)> {
    let session = load_owned(&state, user_id, id).await?;
    let upload = read_upload(&mut multipart).await?;
    if upload.data.is_empty() {
        return Err(ApiError::BadRequest("uploaded file is empty".into()));
    }
    let name = sanitize_name(&upload.name);
    let attachment = if session.agent_profile == DESIGNER_PROFILE {
        to_draft(&state, name, upload.data).await?
    } else {
        to_files(&state, user_id, name, upload).await?
    };
    Ok((StatusCode::CREATED, Json(attachment)))
}

async fn to_files(
    state: &AppState,
    user_id: i32,
    name: String,
    upload: Upload,
) -> ApiResult<Attachment> {
    let dir = chrono::Utc::now().format("uploads/chat/%Y-%m").to_string();
    let taken: HashSet<String> = file::Entity::find()
        .select_only()
        .column(file::Column::Path)
        .filter(file::Column::Path.starts_with(format!("{dir}/")))
        .into_tuple::<String>()
        .all(&state.db)
        .await?
        .into_iter()
        .collect();
    let path = free_path(&dir, &name, |p| taken.contains(p)).ok_or_else(folder_full)?;
    let mimetype = files_repo::resolve_mimetype(upload.mimetype, &path);
    let created = files_repo::create_file(
        &state.db,
        &state.storage,
        user_id,
        NewFile {
            path,
            description: None,
            mimetype,
            data: upload.data,
        },
    )
    .await?;
    broadcast::file_created(&state.ws_hub, &created.model, created.has_thumbnail);
    Ok(Attachment {
        size: created.model.size_bytes as usize,
        path: created.model.path,
        mimetype: created.model.mimetype,
        target: "file",
        file_id: Some(created.model.id),
    })
}

async fn to_draft(state: &AppState, name: String, data: Vec<u8>) -> ApiResult<Attachment> {
    let dir = design_folder(&name).ok_or_else(|| {
        ApiError::Unprocessable(
            "Designer chats take images and fonts only (they go to the draft's \
             assets/img/ and assets/fonts/)"
                .into(),
        )
    })?;
    let draft = state.design.draft(&state.storage).await?;
    let view = state.design.with_baked(&draft.files);
    let path = free_path(dir, &name, |p| view.contains_key(p)).ok_or_else(folder_full)?;
    let size = data.len();
    state
        .design
        .draft_put(&state.storage, &path, Bytes::from(data))
        .await?;
    broadcast::design_draft_changed(&state.ws_hub, &DraftChange::Put { path: &path });
    Ok(Attachment {
        mimetype: file_mimetype(&path),
        path,
        size,
        target: "design",
        file_id: None,
    })
}

/// The `file` field of the form; anything over [`MAX_ATTACHMENT_SIZE`] is
/// refused while streaming, before it is buffered whole.
async fn read_upload(multipart: &mut Multipart) -> ApiResult<Upload> {
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or_default().to_string();
        let mimetype = field.content_type().map(str::to_string);
        let mut data = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            if data.len() + chunk.len() > MAX_ATTACHMENT_SIZE {
                return Err(too_large());
            }
            data.extend_from_slice(&chunk);
        }
        return Ok(Upload {
            name,
            mimetype,
            data,
        });
    }
    Err(ApiError::BadRequest("missing file field".into()))
}

fn multipart_error(e: MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large()
    } else {
        ApiError::BadRequest(format!("multipart error: {}", e.body_text()))
    }
}

fn too_large() -> ApiError {
    ApiError::Detailed {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        message: format!(
            "attachment too large (max {} MB)",
            MAX_ATTACHMENT_SIZE / 1024 / 1024
        ),
        code: "too_large",
        details: Vec::new(),
    }
}

fn folder_full() -> ApiError {
    ApiError::Conflict("too many attachments with this name; rename the file".into())
}

/// A client-supplied filename as one safe, lowercase path segment: only
/// `[a-z0-9_-]` plus one extension dot survive (no separators, no `..`, no
/// hidden files); a name with nothing left becomes `file`.
pub fn sanitize_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i + 1..]),
        _ => (base, ""),
    };
    let mut stem = clean_segment(stem, 80);
    if stem.is_empty() {
        stem = "file".into();
    }
    let ext = clean_segment(ext, 10);
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

fn clean_segment(raw: &str, max: usize) -> String {
    let mut out = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    out.truncate(max);
    out.trim_end_matches('-').to_string()
}

/// `dir/name`, or `dir/stem-2.ext`, `-3`, … — the first one not `taken`.
pub fn free_path(dir: &str, name: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    };
    (1..=MAX_CANDIDATES)
        .map(|n| match n {
            1 => format!("{dir}/{name}"),
            n => format!("{dir}/{stem}-{n}{ext}"),
        })
        .find(|p| !taken(p))
}

/// Where a Designer chat's attachment goes in the draft; `None` refuses it.
pub fn design_folder(name: &str) -> Option<&'static str> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    if IMAGE_EXTENSIONS.contains(&ext) {
        Some("assets/img")
    } else if FONT_EXTENSIONS.contains(&ext) {
        Some("assets/fonts")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_name_keeps_a_safe_lowercase_segment() {
        assert_eq!(
            sanitize_name("Screenshot 2026-10-10 at 12.00.png"),
            "screenshot-2026-10-10-at-12-00.png"
        );
        assert_eq!(sanitize_name("Obrázek Č.1.JPG"), "obr-zek-1.jpg");
        assert_eq!(sanitize_name("my_font.WOFF2"), "my_font.woff2");
        assert_eq!(sanitize_name("README"), "readme");
    }

    #[test]
    fn sanitize_name_strips_directories_and_traversal() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("C:\\Users\\me\\cat.gif"), "cat.gif");
        assert_eq!(sanitize_name(".."), "file");
        assert_eq!(sanitize_name(".env"), "env");
        assert_eq!(sanitize_name(""), "file");
        assert_eq!(sanitize_name("!!!.png"), "file.png");
    }

    #[test]
    fn sanitize_name_bounds_the_length() {
        let name = sanitize_name(&format!("{}.{}", "a".repeat(300), "b".repeat(30)));
        assert_eq!(name, format!("{}.{}", "a".repeat(80), "b".repeat(10)));
    }

    #[test]
    fn free_path_suffixes_collisions_before_the_extension() {
        let taken = ["d/a.png", "d/a-2.png"];
        let free = free_path("d", "a.png", |p| taken.contains(&p));
        assert_eq!(free.as_deref(), Some("d/a-3.png"));
        assert_eq!(
            free_path("d", "b.png", |_| false).as_deref(),
            Some("d/b.png")
        );
        assert_eq!(
            free_path("d", "readme", |p| p == "d/readme").as_deref(),
            Some("d/readme-2")
        );
        assert_eq!(free_path("d", "a.png", |_| true), None);
    }

    #[test]
    fn design_folder_routes_images_and_fonts_only() {
        assert_eq!(design_folder("logo.png"), Some("assets/img"));
        assert_eq!(design_folder("icon.svg"), Some("assets/img"));
        assert_eq!(design_folder("inter.woff2"), Some("assets/fonts"));
        assert_eq!(design_folder("notes.txt"), None);
        assert_eq!(design_folder("readme"), None);
    }
}
