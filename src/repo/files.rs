use std::collections::HashSet;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::entity::{file, file_thumbnail};
use crate::files::{hash_blob, make_thumbnail, put_blob};
use crate::path_util;
use crate::repo::pages::ChildRow;

pub async fn list_children(
    db: &DatabaseConnection,
    prefix: &str,
    limit: u64,
) -> Result<Vec<ChildRow>, DbErr> {
    let prefix_len = prefix.len() as i32;
    let like_pattern = if prefix.is_empty() {
        "%".to_string()
    } else {
        format!("{prefix}%")
    };
    let sql = "SELECT
            split_part(substr(path, $1::int + 1), '/', 1) AS name,
            bool_or(strpos(substr(path, $1::int + 1), '/') > 0) AS has_descendants,
            count(*) FILTER (WHERE strpos(substr(path, $1::int + 1), '/') > 0) AS descendant_count,
            bool_or(strpos(substr(path, $1::int + 1), '/') = 0) AS has_leaf,
            max(path) FILTER (WHERE strpos(substr(path, $1::int + 1), '/') = 0) AS leaf_title
        FROM files
        WHERE path LIKE $2
        GROUP BY 1
        ORDER BY 1
        LIMIT $3";
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![
            Value::from(prefix_len),
            Value::from(like_pattern),
            Value::from(limit as i64),
        ],
    );
    let rows = db.query_all(stmt).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get_by("name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        out.push(ChildRow {
            name,
            has_descendants: row.try_get_by("has_descendants").unwrap_or(false),
            descendant_count: row.try_get_by("descendant_count").unwrap_or(0),
            has_leaf: row.try_get_by("has_leaf").unwrap_or(false),
            leaf_title: row.try_get_by("leaf_title").ok().flatten(),
        });
    }
    Ok(out)
}

pub struct NewFile {
    pub path: String,
    pub description: Option<String>,
    pub mimetype: String,
    pub data: Vec<u8>,
}

pub struct CreatedFile {
    pub model: file::Model,
    pub has_thumbnail: bool,
}

pub struct FileWithThumb {
    pub model: file::Model,
    pub has_thumbnail: bool,
}

pub struct FileMetaUpdate {
    pub path: String,
    pub description: Option<String>,
    pub mimetype: Option<String>,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum FileSaveError {
    EmptyPath,
    EmptyData,
    Db(DbErr),
}

impl std::fmt::Display for FileSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath => write!(f, "path is required"),
            Self::EmptyData => write!(f, "decoded data is empty"),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<DbErr> for FileSaveError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub fn title_from_path(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Infer a mimetype from a path's extension when none was supplied by the
/// caller. The site's own directive formats (`.pgn`/`.mmd`/`.fen`) get
/// explicit mimetypes since `mime_guess` doesn't know them; everything else
/// falls back to `mime_guess`, then `application/octet-stream`.
pub fn infer_mimetype(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pgn" => "application/x-chess-pgn".to_string(),
        "mmd" | "mermaid" => "text/vnd.mermaid".to_string(),
        "fen" => "text/plain".to_string(),
        _ => mime_guess::from_path(path)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string()),
    }
}

/// Whether a mimetype's bytes are safe to decode and return as UTF-8 text —
/// covers the site's own text-ish directive formats (`.pgn`/`.mmd`/`.fen`/
/// `.json`, per `infer_mimetype`/`embed_hint` above) plus generic `text/*`.
pub fn is_text_content(mimetype: &str) -> bool {
    mimetype.starts_with("text/")
        || mimetype == "application/json"
        || mimetype == "application/x-chess-pgn"
}

/// Suggest the markdown directive to embed a newly created file, based on its
/// extension/mimetype — `<image>` only makes sense for `image/*` blobs; a
/// `.pgn`/`.mmd`/`.fen`/`.json` file needs its own type-specific directive to
/// render as a board/diagram/table instead of a broken `<img>`.
pub fn embed_hint(path: &str, mimetype: &str, id: i32) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pgn" => format!(r#"<pgn id="{id}">"#),
        "mmd" | "mermaid" => format!(r#"<mermaid id="{id}">"#),
        "fen" => format!(r#"<fen id="{id}">"#),
        "json" => format!(r#"<json id="{id}" query=".">"#),
        _ if mimetype.starts_with("image/") => format!(r#"<image id="{id}">"#),
        _ => format!(r#"<file id="{id}">"#),
    }
}

pub async fn create_file(
    db: &DatabaseConnection,
    user_id: i32,
    input: NewFile,
) -> Result<CreatedFile, FileSaveError> {
    let path = path_util::normalize(&input.path);
    if path.is_empty() {
        return Err(FileSaveError::EmptyPath);
    }
    if input.data.is_empty() {
        return Err(FileSaveError::EmptyData);
    }
    let hash = hash_blob(&input.data);
    let size_bytes = input.data.len() as i64;
    put_blob(db, &hash, &input.data).await?;

    let now = chrono::Utc::now().fixed_offset();
    let model = file::ActiveModel {
        hash: Set(hash),
        mimetype: Set(input.mimetype.clone()),
        path: Set(path),
        description: Set(input.description.filter(|s| !s.is_empty())),
        size_bytes: Set(size_bytes),
        created_at: Set(now),
        created_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await?;

    let mut has_thumbnail = false;
    if let Some(thumb) = make_thumbnail(&input.data, &input.mimetype) {
        let thumb_hash = hash_blob(&thumb.data);
        if put_blob(db, &thumb_hash, &thumb.data).await.is_ok() {
            let thumb_row = file_thumbnail::ActiveModel {
                file_id: Set(model.id),
                hash: Set(thumb_hash),
                width: Set(thumb.width as i32),
                height: Set(thumb.height as i32),
                mimetype: Set(thumb.mimetype.to_string()),
                created_at: Set(now),
            };
            if thumb_row.insert(db).await.is_ok() {
                has_thumbnail = true;
            }
        }
    }

    Ok(CreatedFile {
        model,
        has_thumbnail,
    })
}

pub async fn list_with_thumbnails(
    db: &DatabaseConnection,
    mime_prefix: Option<&str>,
) -> Result<Vec<FileWithThumb>, DbErr> {
    let mut select = file::Entity::find().order_by_desc(file::Column::CreatedAt);
    if let Some(prefix) = mime_prefix.filter(|s| !s.is_empty()) {
        select = select.filter(file::Column::Mimetype.starts_with(prefix));
    }
    let rows = select.all(db).await?;

    let ids: Vec<i32> = rows.iter().map(|f| f.id).collect();
    let thumb_ids: HashSet<i32> = if ids.is_empty() {
        HashSet::new()
    } else {
        file_thumbnail::Entity::find()
            .filter(file_thumbnail::Column::FileId.is_in(ids))
            .all(db)
            .await?
            .into_iter()
            .map(|t| t.file_id)
            .collect()
    };

    Ok(rows
        .into_iter()
        .map(|f| {
            let has_thumbnail = thumb_ids.contains(&f.id);
            FileWithThumb {
                model: f,
                has_thumbnail,
            }
        })
        .collect())
}

pub async fn find_with_thumbnail(
    db: &DatabaseConnection,
    id: i32,
) -> Result<Option<FileWithThumb>, DbErr> {
    let Some(model) = file::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let has_thumbnail = has_thumbnail(db, id).await?;
    Ok(Some(FileWithThumb {
        model,
        has_thumbnail,
    }))
}

pub async fn find_by_hash(
    db: &DatabaseConnection,
    hash: &str,
) -> Result<Option<file::Model>, DbErr> {
    file::Entity::find()
        .filter(file::Column::Hash.eq(hash))
        .one(db)
        .await
}

pub async fn has_thumbnail(db: &DatabaseConnection, file_id: i32) -> Result<bool, DbErr> {
    Ok(file_thumbnail::Entity::find_by_id(file_id)
        .one(db)
        .await?
        .is_some())
}

pub async fn update_metadata(
    db: &DatabaseConnection,
    id: i32,
    update: FileMetaUpdate,
) -> Result<Option<FileWithThumb>, FileSaveError> {
    let Some(model) = file::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let mut active: file::ActiveModel = model.into();
    active.path = Set(path_util::normalize(&update.path));
    active.description = Set(update.description.filter(|s| !s.is_empty()));

    let new_data = match update.data {
        Some(data) if data.is_empty() => return Err(FileSaveError::EmptyData),
        Some(data) => {
            let hash = hash_blob(&data);
            put_blob(db, &hash, &data).await?;
            active.hash = Set(hash);
            active.size_bytes = Set(data.len() as i64);
            Some(data)
        }
        None => None,
    };
    if let Some(mimetype) = update.mimetype {
        active.mimetype = Set(mimetype);
    }

    let updated = active.update(db).await?;

    let has_thumbnail = if let Some(data) = new_data {
        file_thumbnail::Entity::delete_by_id(id).exec(db).await?;
        let mut has_thumbnail = false;
        if let Some(thumb) = make_thumbnail(&data, &updated.mimetype) {
            let thumb_hash = hash_blob(&thumb.data);
            if put_blob(db, &thumb_hash, &thumb.data).await.is_ok() {
                let now = chrono::Utc::now().fixed_offset();
                let thumb_row = file_thumbnail::ActiveModel {
                    file_id: Set(id),
                    hash: Set(thumb_hash),
                    width: Set(thumb.width as i32),
                    height: Set(thumb.height as i32),
                    mimetype: Set(thumb.mimetype.to_string()),
                    created_at: Set(now),
                };
                if thumb_row.insert(db).await.is_ok() {
                    has_thumbnail = true;
                }
            }
        }
        has_thumbnail
    } else {
        has_thumbnail(db, id).await?
    };

    Ok(Some(FileWithThumb {
        model: updated,
        has_thumbnail,
    }))
}

pub async fn delete_by_id(db: &DatabaseConnection, id: i32) -> Result<bool, DbErr> {
    let res = file::Entity::delete_by_id(id).exec(db).await?;
    Ok(res.rows_affected > 0)
}

#[cfg(test)]
mod tests {
    use super::{embed_hint, infer_mimetype, is_text_content};

    #[test]
    fn pgn_hints_pgn_directive() {
        assert_eq!(
            embed_hint("game.pgn", "application/octet-stream", 1),
            r#"<pgn id="1">"#
        );
    }

    #[test]
    fn mermaid_hints_mermaid_directive() {
        assert_eq!(
            embed_hint("diagrams/flow.mmd", "text/plain", 2),
            r#"<mermaid id="2">"#
        );
    }

    #[test]
    fn fen_hints_fen_directive() {
        assert_eq!(
            embed_hint("opening.fen", "application/x-chess-fen", 3),
            r#"<fen id="3">"#
        );
    }

    #[test]
    fn json_hints_json_directive_with_query_placeholder() {
        assert_eq!(
            embed_hint("data/stats.json", "application/json", 4),
            r#"<json id="4" query=".">"#
        );
    }

    #[test]
    fn image_mimetype_hints_image_directive() {
        assert_eq!(
            embed_hint("photo.jpg", "image/jpeg", 5),
            r#"<image id="5">"#
        );
    }

    #[test]
    fn unknown_type_hints_file_directive() {
        assert_eq!(embed_hint("notes.txt", "text/plain", 6), r#"<file id="6">"#);
    }

    #[test]
    fn extension_wins_over_generic_mimetype() {
        assert_eq!(
            embed_hint("game.pgn", "application/octet-stream", 7),
            r#"<pgn id="7">"#
        );
    }

    #[test]
    fn infers_pgn_mimetype() {
        assert_eq!(infer_mimetype("game.pgn"), "application/x-chess-pgn");
    }

    #[test]
    fn infers_mermaid_mimetype() {
        assert_eq!(infer_mimetype("diagrams/flow.mmd"), "text/vnd.mermaid");
    }

    #[test]
    fn infers_fen_mimetype() {
        assert_eq!(infer_mimetype("opening.fen"), "text/plain");
    }

    #[test]
    fn infers_known_extension_via_mime_guess() {
        assert_eq!(infer_mimetype("photo.jpg"), "image/jpeg");
    }

    #[test]
    fn falls_back_to_octet_stream_for_unknown_extension() {
        assert_eq!(infer_mimetype("blob.bin"), "application/octet-stream");
    }

    #[test]
    fn text_plain_is_text_content() {
        assert!(is_text_content("text/plain"));
    }

    #[test]
    fn json_is_text_content() {
        assert!(is_text_content("application/json"));
    }

    #[test]
    fn pgn_is_text_content() {
        assert!(is_text_content("application/x-chess-pgn"));
    }

    #[test]
    fn image_is_not_text_content() {
        assert!(!is_text_content("image/png"));
    }
}
