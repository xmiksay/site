use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, Value,
};

use crate::entity::page;
use crate::path_util;
use crate::routes::revision;

#[derive(Debug, Clone)]
pub struct ChildRow {
    pub name: String,
    pub has_descendants: bool,
    pub descendant_count: i64,
    pub has_leaf: bool,
    pub leaf_title: Option<String>,
}

pub async fn list_children(
    db: &DatabaseConnection,
    prefix: &str,
    include_private: bool,
    limit: u64,
) -> Result<Vec<ChildRow>, DbErr> {
    let prefix_len = prefix.len() as i32;
    let like_pattern = if prefix.is_empty() {
        "%".to_string()
    } else {
        format!("{prefix}%")
    };
    let private_clause = if include_private {
        "TRUE"
    } else {
        "private = FALSE"
    };
    let sql = format!(
        "SELECT
            split_part(substr(path, $1::int + 1), '/', 1) AS name,
            bool_or(strpos(substr(path, $1::int + 1), '/') > 0) AS has_descendants,
            count(*) FILTER (WHERE strpos(substr(path, $1::int + 1), '/') > 0) AS descendant_count,
            bool_or(strpos(substr(path, $1::int + 1), '/') = 0) AS has_leaf,
            max(COALESCE(summary, path)) FILTER (WHERE strpos(substr(path, $1::int + 1), '/') = 0) AS leaf_title
        FROM pages
        WHERE path LIKE $2 AND ({private_clause})
        GROUP BY 1
        ORDER BY 1
        LIMIT $3"
    );
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

#[derive(Default)]
pub struct PageUpdate {
    pub markdown: Option<String>,
    pub summary: Option<String>,
    pub tag_ids: Option<Vec<i32>>,
    pub private: Option<bool>,
}

/// The "nothing to update" guard shared by every edge that upserts a page by
/// path (MCP `edit_page`, the AI assistant's `edit_page` tool) — checked
/// against the *raw* caller-provided fields, before `tag_names` is resolved
/// to `tag_ids`, so an explicit empty `tag_names: []` still counts as "the
/// caller provided a field" (existing behavior: it resolves to no tag change,
/// not a rejected call).
pub fn validate_page_edit_fields(
    markdown: &Option<String>,
    summary: &Option<String>,
    tag_names: &Option<Vec<String>>,
    private: &Option<bool>,
) -> Result<(), &'static str> {
    if markdown.is_none() && summary.is_none() && tag_names.is_none() && private.is_none() {
        Err("Nothing to update — provide markdown, summary, tag_names, or private")
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum PageSaveError {
    EmptyPath,
    Db(DbErr),
}

impl std::fmt::Display for PageSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath => write!(f, "path is required"),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<DbErr> for PageSaveError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub struct PageNew {
    pub path: String,
    pub markdown: String,
    pub summary: Option<String>,
    pub tag_ids: Vec<i32>,
    pub private: bool,
}

pub enum PageSort {
    PathAsc,
    PathDesc,
    ModifiedAsc,
    ModifiedDesc,
}

impl PageSort {
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("path") => Self::PathAsc,
            Some("path_desc") => Self::PathDesc,
            Some("modified_asc") => Self::ModifiedAsc,
            _ => Self::ModifiedDesc,
        }
    }
}

pub async fn list_paths(
    db: &DatabaseConnection,
    prefix: Option<&str>,
    limit: Option<u64>,
) -> Result<Vec<String>, DbErr> {
    let mut select = page::Entity::find()
        .select_only()
        .column(page::Column::Path)
        .order_by_asc(page::Column::Path);
    if let Some(prefix) = prefix.filter(|s| !s.is_empty()) {
        select = select.filter(page::Column::Path.starts_with(prefix));
    }
    if let Some(limit) = limit {
        select = select.limit(limit);
    }
    select.into_tuple::<String>().all(db).await
}

pub async fn find_by_path(
    db: &DatabaseConnection,
    path: &str,
) -> Result<Option<page::Model>, DbErr> {
    let normalized = path_util::normalize(path);
    page::Entity::find()
        .filter(page::Column::Path.eq(normalized))
        .one(db)
        .await
}

pub async fn list_all(db: &DatabaseConnection, sort: PageSort) -> Result<Vec<page::Model>, DbErr> {
    let select = page::Entity::find();
    let select = match sort {
        PageSort::PathAsc => select.order_by_asc(page::Column::Path),
        PageSort::PathDesc => select.order_by_desc(page::Column::Path),
        PageSort::ModifiedAsc => select.order_by_asc(page::Column::ModifiedAt),
        PageSort::ModifiedDesc => select.order_by_desc(page::Column::ModifiedAt),
    };
    select.all(db).await
}

pub async fn create(
    db: &DatabaseConnection,
    user_id: i32,
    input: PageNew,
) -> Result<page::Model, PageSaveError> {
    let path = path_util::normalize(&input.path);
    if path.is_empty() {
        return Err(PageSaveError::EmptyPath);
    }
    let now = chrono::Utc::now().fixed_offset();
    Ok(page::ActiveModel {
        path: Set(path),
        summary: Set(input.summary.filter(|s| !s.is_empty())),
        markdown: Set(input.markdown),
        tag_ids: Set(input.tag_ids),
        private: Set(input.private),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn replace(
    db: &DatabaseConnection,
    user_id: i32,
    id: i32,
    input: PageNew,
) -> Result<Option<page::Model>, PageSaveError> {
    let normalized_path = path_util::normalize(&input.path);
    if normalized_path.is_empty() {
        return Err(PageSaveError::EmptyPath);
    }
    let Some(model) = page::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let now = chrono::Utc::now().fixed_offset();
    let old_markdown = model.markdown.clone();
    let new_markdown = input.markdown.clone();

    let mut active: page::ActiveModel = model.into();
    active.path = Set(normalized_path);
    active.summary = Set(input.summary.filter(|s| !s.is_empty()));
    active.markdown = Set(input.markdown);
    active.tag_ids = Set(input.tag_ids);
    active.private = Set(input.private);
    active.modified_at = Set(now);
    active.modified_by = Set(user_id);
    let updated = active.update(db).await?;

    revision::create_revision_if_changed(db, id, &old_markdown, &new_markdown, user_id).await;
    Ok(Some(updated))
}

pub enum UpsertOutcome {
    Created(page::Model),
    Updated(page::Model),
}

/// Upsert by path, applying only the fields present in `update`. Stores a
/// revision diff if markdown changed. On insert, missing fields default to
/// empty / true (private).
pub async fn upsert_by_path(
    db: &DatabaseConnection,
    user_id: i32,
    path: &str,
    update: PageUpdate,
) -> Result<UpsertOutcome, PageSaveError> {
    let now = chrono::Utc::now().fixed_offset();
    let path = path_util::normalize(path);
    if path.is_empty() {
        return Err(PageSaveError::EmptyPath);
    }
    let existing = find_by_path(db, &path).await?;

    match existing {
        Some(model) => {
            let old_markdown = model.markdown.clone();
            let new_markdown_opt = update.markdown.clone();
            let page_id = model.id;
            let mut active: page::ActiveModel = model.into();
            if let Some(md) = update.markdown {
                active.markdown = Set(md);
            }
            if let Some(s) = update.summary {
                active.summary = Set(Some(s).filter(|s| !s.is_empty()));
            }
            if let Some(ids) = update.tag_ids {
                active.tag_ids = Set(ids);
            }
            if let Some(p) = update.private {
                active.private = Set(p);
            }
            active.modified_at = Set(now);
            active.modified_by = Set(user_id);
            let updated = active.update(db).await?;

            if let Some(new_md) = new_markdown_opt {
                revision::create_revision_if_changed(db, page_id, &old_markdown, &new_md, user_id)
                    .await;
            }
            Ok(UpsertOutcome::Updated(updated))
        }
        None => {
            let saved = page::ActiveModel {
                path: Set(path),
                summary: Set(update.summary.filter(|s| !s.is_empty())),
                markdown: Set(update.markdown.unwrap_or_default()),
                tag_ids: Set(update.tag_ids.unwrap_or_default()),
                private: Set(update.private.unwrap_or(true)),
                created_at: Set(now),
                created_by: Set(user_id),
                modified_at: Set(now),
                modified_by: Set(user_id),
                ..Default::default()
            }
            .insert(db)
            .await?;
            Ok(UpsertOutcome::Created(saved))
        }
    }
}

pub async fn delete_by_id(db: &DatabaseConnection, id: i32) -> Result<bool, DbErr> {
    let res = page::Entity::delete_by_id(id).exec(db).await?;
    Ok(res.rows_affected > 0)
}

/// Delete a page by path, returning its id (for a WS broadcast) if it existed.
pub async fn delete_by_path(db: &DatabaseConnection, path: &str) -> Result<Option<i32>, DbErr> {
    let Some(model) = find_by_path(db, path).await? else {
        return Ok(None);
    };
    page::Entity::delete_by_id(model.id).exec(db).await?;
    Ok(Some(model.id))
}
