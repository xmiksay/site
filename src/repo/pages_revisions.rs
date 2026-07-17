//! Page revision history — split out of `repo/pages.rs` (which was already
//! over the 400-line file cap) as the CRUD/search file's most self-contained
//! seam: reading/restoring a past revision, as opposed to the current row.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::entity::{page, page_revision};
use crate::routes::revision::{self, ReconstructError};

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
