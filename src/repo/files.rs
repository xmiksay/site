use std::collections::HashSet;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::entity::{file, file_thumbnail};
use crate::files::make_thumbnail;
use crate::path_util;
use crate::repo::pages::ChildRow;
use crate::storage::{self, Storage};

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
    Storage(storage::Error),
}

impl std::fmt::Display for FileSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath => write!(f, "path is required"),
            Self::EmptyData => write!(f, "decoded data is empty"),
            Self::Db(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl From<DbErr> for FileSaveError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<storage::Error> for FileSaveError {
    fn from(e: storage::Error) -> Self {
        Self::Storage(e)
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
    storage: &Storage,
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
    let size_bytes = input.data.len() as i64;
    let hash = storage.put_blob(&input.data).await?;

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

    let has_thumbnail = store_thumbnail(db, storage, model.id, &input.data, &input.mimetype).await;

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
    storage: &Storage,
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
            active.hash = Set(storage.put_blob(&data).await?);
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
        store_thumbnail(db, storage, id, &data, &updated.mimetype).await
    } else {
        has_thumbnail(db, id).await?
    };

    Ok(Some(FileWithThumb {
        model: updated,
        has_thumbnail,
    }))
}

/// Best effort: a file without a thumbnail is still a valid file, so any
/// failure here only means `has_thumbnail: false`.
async fn store_thumbnail(
    db: &DatabaseConnection,
    storage: &Storage,
    file_id: i32,
    data: &[u8],
    mimetype: &str,
) -> bool {
    let Some(thumb) = make_thumbnail(data, mimetype) else {
        return false;
    };
    let hash = match storage.put_blob(&thumb.data).await {
        Ok(hash) => hash,
        Err(e) => {
            tracing::warn!(file_id, "storing thumbnail failed: {e}");
            return false;
        }
    };
    file_thumbnail::ActiveModel {
        file_id: Set(file_id),
        hash: Set(hash),
        width: Set(thumb.width as i32),
        height: Set(thumb.height as i32),
        mimetype: Set(thumb.mimetype.to_string()),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .is_ok()
}

pub async fn delete_by_id(db: &DatabaseConnection, id: i32) -> Result<bool, DbErr> {
    let res = file::Entity::delete_by_id(id).exec(db).await?;
    Ok(res.rows_affected > 0)
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
