use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, Value,
};

use crate::entity::{page, page_revision, tag};
use crate::path_util;
use crate::routes::revision::{self, ReconstructError};

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

pub struct SearchResult {
    pub pages: Vec<page::Model>,
    pub total: u64,
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

#[derive(Debug)]
pub enum SearchError {
    Db(DbErr),
    UnknownTag,
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "{e}"),
            Self::UnknownTag => write!(f, "unknown tag"),
        }
    }
}

impl From<DbErr> for SearchError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub async fn search(
    db: &DatabaseConnection,
    prefix: Option<&str>,
    tag_name: Option<&str>,
    q: Option<&str>,
    include_private: bool,
    limit: u64,
    offset: u64,
) -> Result<SearchResult, SearchError> {
    let prefix = prefix.filter(|s| !s.is_empty());
    let tag_name = tag_name.filter(|s| !s.is_empty());
    let q_clean = q.map(|s| s.trim()).filter(|s| !s.is_empty());

    // Resolve tag id up front so we can short-circuit unknown tag.
    let tag_id = if let Some(name) = tag_name {
        match tag::Entity::find()
            .filter(tag::Column::Name.eq(name))
            .one(db)
            .await?
        {
            Some(t) => Some(t.id),
            None => return Err(SearchError::UnknownTag),
        }
    } else {
        None
    };

    // Build the WHERE clause and shared parameter list.
    let mut where_sql = String::from("TRUE");
    let mut values: Vec<Value> = Vec::new();
    let mut next_idx = 1usize;
    let mut placeholder = |sql: &mut String, values: &mut Vec<Value>, v: Value| -> usize {
        values.push(v);
        let i = next_idx;
        next_idx += 1;
        let _ = sql;
        i
    };

    if !include_private {
        where_sql.push_str(" AND private = FALSE");
    }
    if let Some(p) = prefix {
        let i = placeholder(&mut where_sql, &mut values, format!("{p}%").into());
        where_sql.push_str(&format!(" AND path LIKE ${i}"));
    }
    if let Some(id) = tag_id {
        let i = placeholder(&mut where_sql, &mut values, id.into());
        where_sql.push_str(&format!(" AND ${i} = ANY(tag_ids)"));
    }
    let q_idx = if let Some(q_str) = q_clean {
        let v: Value = q_str.to_string().into();
        let i1 = placeholder(&mut where_sql, &mut values, v.clone());
        let i2 = placeholder(&mut where_sql, &mut values, v.clone());
        let i3 = placeholder(&mut where_sql, &mut values, v);
        where_sql.push_str(&format!(
            " AND (search_tsv @@ plainto_tsquery('simple', f_unaccent(${i1})) \
                   OR f_unaccent(path) ILIKE '%' || f_unaccent(${i2}) || '%' \
                   OR EXISTS (SELECT 1 FROM tags \
                              WHERE tags.id = ANY(pages.tag_ids) \
                                AND f_unaccent(tags.name) ILIKE '%' || f_unaccent(${i3}) || '%'))"
        ));
        Some((i1, i3))
    } else {
        None
    };

    // Count
    let count_sql = format!("SELECT count(*) AS c FROM pages WHERE {where_sql}");
    let count_stmt = Statement::from_sql_and_values(DbBackend::Postgres, count_sql, values.clone());
    let total: i64 = db
        .query_one(count_stmt)
        .await?
        .and_then(|r| r.try_get_by::<i64, _>("c").ok())
        .unwrap_or(0);
    if total == 0 {
        return Ok(SearchResult {
            pages: vec![],
            total: 0,
        });
    }

    // Rows
    let order_sql = match q_idx {
        Some((i_rank, i_tag)) => format!(
            "ts_rank(search_tsv, plainto_tsquery('simple', f_unaccent(${i_rank}))) \
             + CASE WHEN EXISTS (SELECT 1 FROM tags \
                                 WHERE tags.id = ANY(pages.tag_ids) \
                                   AND f_unaccent(tags.name) ILIKE '%' || f_unaccent(${i_tag}) || '%') \
                    THEN 0.5 ELSE 0 END DESC, modified_at DESC"
        ),
        None => "modified_at DESC".to_string(),
    };
    let i_limit = next_idx;
    let i_offset = next_idx + 1;
    values.push((limit as i64).into());
    values.push((offset as i64).into());

    let select_sql = format!(
        "SELECT * FROM pages WHERE {where_sql} ORDER BY {order_sql} LIMIT ${i_limit} OFFSET ${i_offset}"
    );
    let stmt = Statement::from_sql_and_values(DbBackend::Postgres, select_sql, values);
    let pages = page::Entity::find().from_raw_sql(stmt).all(db).await?;
    Ok(SearchResult {
        pages,
        total: total as u64,
    })
}

pub async fn create(
    db: &DatabaseConnection,
    user_id: i32,
    input: PageNew,
) -> Result<page::Model, DbErr> {
    let now = chrono::Utc::now().fixed_offset();
    page::ActiveModel {
        path: Set(path_util::normalize(&input.path)),
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
    .await
}

pub async fn replace(
    db: &DatabaseConnection,
    user_id: i32,
    id: i32,
    input: PageNew,
) -> Result<Option<page::Model>, DbErr> {
    let Some(model) = page::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let now = chrono::Utc::now().fixed_offset();
    let old_markdown = model.markdown.clone();
    let new_markdown = input.markdown.clone();

    let mut active: page::ActiveModel = model.into();
    active.path = Set(path_util::normalize(&input.path));
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
) -> Result<UpsertOutcome, DbErr> {
    let now = chrono::Utc::now().fixed_offset();
    let path = path_util::normalize(path);
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

pub async fn delete_by_path(db: &DatabaseConnection, path: &str) -> Result<bool, DbErr> {
    let res = page::Entity::delete_many()
        .filter(page::Column::Path.eq(path_util::normalize(path)))
        .exec(db)
        .await?;
    Ok(res.rows_affected > 0)
}

/// Load a single revision belonging to `page_id` (None if it isn't that page's).
pub async fn get_revision(
    db: &DatabaseConnection,
    page_id: i32,
    rev_id: i32,
) -> Result<Option<page_revision::Model>, DbErr> {
    page_revision::Entity::find_by_id(rev_id)
        .filter(page_revision::Column::PageId.eq(page_id))
        .one(db)
        .await
}

pub async fn list_revisions(
    db: &DatabaseConnection,
    page_id: i32,
) -> Result<Vec<page_revision::Model>, DbErr> {
    page_revision::Entity::find()
        .filter(page_revision::Column::PageId.eq(page_id))
        .order_by_desc(page_revision::Column::CreatedAt)
        .all(db)
        .await
}

#[derive(Debug)]
pub enum RestoreError {
    NotFound,
    Reconstruct(ReconstructError),
    Db(DbErr),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Page not found"),
            Self::Reconstruct(e) => write!(f, "{e}"),
            Self::Db(e) => write!(f, "Database error: {e}"),
        }
    }
}

impl From<DbErr> for RestoreError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub async fn restore_revision(
    db: &DatabaseConnection,
    user_id: i32,
    page_id: i32,
    revision_id: i32,
) -> Result<page::Model, RestoreError> {
    let model = page::Entity::find_by_id(page_id)
        .one(db)
        .await?
        .ok_or(RestoreError::NotFound)?;

    let restored = revision::reconstruct_at_revision(db, page_id, revision_id, &model.markdown)
        .await
        .map_err(RestoreError::Reconstruct)?;

    let old_markdown = model.markdown.clone();
    let now = chrono::Utc::now().fixed_offset();
    let mut active: page::ActiveModel = model.into();
    active.markdown = Set(restored.clone());
    active.modified_at = Set(now);
    active.modified_by = Set(user_id);
    let updated = active.update(db).await?;

    revision::create_revision_if_changed(db, page_id, &old_markdown, &restored, user_id).await;
    Ok(updated)
}
