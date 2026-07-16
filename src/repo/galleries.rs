use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, Value,
};

use crate::entity::gallery;
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
            max(title) FILTER (WHERE strpos(substr(path, $1::int + 1), '/') = 0) AS leaf_title
        FROM galleries
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

pub struct GalleryInput {
    pub path: String,
    pub title: String,
    pub description: Option<String>,
    pub file_ids: Vec<i32>,
}

pub async fn list_all(db: &DatabaseConnection) -> Result<Vec<gallery::Model>, DbErr> {
    gallery::Entity::find()
        .order_by_desc(gallery::Column::CreatedAt)
        .all(db)
        .await
}

pub async fn list_paths(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
    gallery::Entity::find()
        .select_only()
        .column(gallery::Column::Path)
        .order_by_asc(gallery::Column::Path)
        .into_tuple::<String>()
        .all(db)
        .await
}

pub async fn find_by_id(db: &DatabaseConnection, id: i32) -> Result<Option<gallery::Model>, DbErr> {
    gallery::Entity::find_by_id(id).one(db).await
}

pub async fn find_by_path(
    db: &DatabaseConnection,
    path: &str,
) -> Result<Option<gallery::Model>, DbErr> {
    gallery::Entity::find()
        .filter(gallery::Column::Path.eq(path_util::normalize(path)))
        .one(db)
        .await
}

pub async fn create_gallery(
    db: &DatabaseConnection,
    user_id: i32,
    input: GalleryInput,
) -> Result<gallery::Model, DbErr> {
    gallery::ActiveModel {
        path: Set(path_util::normalize(&input.path)),
        title: Set(input.title),
        description: Set(input.description.filter(|s| !s.is_empty())),
        file_ids: Set(input.file_ids),
        created_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await
}

pub async fn update_gallery(
    db: &DatabaseConnection,
    id: i32,
    input: GalleryInput,
) -> Result<Option<gallery::Model>, DbErr> {
    let Some(model) = gallery::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let mut active: gallery::ActiveModel = model.into();
    active.path = Set(path_util::normalize(&input.path));
    active.title = Set(input.title);
    active.description = Set(input.description.filter(|s| !s.is_empty()));
    active.file_ids = Set(input.file_ids);
    Ok(Some(active.update(db).await?))
}

pub async fn delete_by_id(db: &DatabaseConnection, id: i32) -> Result<bool, DbErr> {
    let res = gallery::Entity::delete_by_id(id).exec(db).await?;
    Ok(res.rows_affected > 0)
}
