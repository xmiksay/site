//! Page fulltext/prefix/tag search — split out of `repo/pages.rs` (which was
//! already over the 400-line file cap) as its most self-contained seam: the
//! raw-SQL ranked query, as opposed to plain CRUD.

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, QueryFilter,
    Statement, Value,
};

use crate::entity::{page, tag};

pub struct SearchResult {
    pub pages: Vec<page::Model>,
    pub total: u64,
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
