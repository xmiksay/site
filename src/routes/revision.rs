use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::entity::{page_revision, page_revision::Entity as PageRevision};

/// Store a reversed diff (new → old) if content actually changed.
pub async fn create_revision_if_changed(
    db: &DatabaseConnection,
    page_id: i32,
    old_markdown: &str,
    new_markdown: &str,
    user_id: i32,
) {
    if old_markdown == new_markdown {
        return;
    }

    let patch = diffy::create_patch(new_markdown, old_markdown).to_string();

    let now = chrono::Utc::now().fixed_offset();
    let revision = page_revision::ActiveModel {
        page_id: Set(page_id),
        patch: Set(patch),
        created_at: Set(now),
        created_by: Set(user_id),
        ..Default::default()
    };

    if let Err(e) = revision.insert(db).await {
        tracing::error!("Failed to store page revision: {e}");
    }
}

/// Reconstruct page content at a given revision by applying reversed patches
/// from newest down to the target revision (inclusive).
///
/// Revisions store reversed diffs (new → old). Applying them sequentially
/// from the most recent walks the content back through history.
pub async fn reconstruct_at_revision(
    db: &DatabaseConnection,
    page_id: i32,
    revision_id: i32,
    current_markdown: &str,
) -> Result<String, ReconstructError> {
    // Load all revisions newer than or equal to target, newest first
    let revisions = PageRevision::find()
        .filter(page_revision::Column::PageId.eq(page_id))
        .filter(page_revision::Column::Id.gte(revision_id))
        .order_by_desc(page_revision::Column::Id)
        .all(db)
        .await
        .map_err(ReconstructError::Db)?;

    if revisions.is_empty() {
        return Err(ReconstructError::NotFound);
    }

    let mut content = current_markdown.to_string();
    for rev in &revisions {
        let patch = diffy::Patch::from_str(&rev.patch)
            .map_err(|e| ReconstructError::PatchParse(rev.id, e.to_string()))?;
        content = diffy::apply(&content, &patch)
            .map_err(|e| ReconstructError::PatchApply(rev.id, e.to_string()))?;
    }

    Ok(content)
}

#[derive(Debug)]
pub enum ReconstructError {
    Db(DbErr),
    NotFound,
    PatchParse(i32, String),
    PatchApply(i32, String),
}

impl std::fmt::Display for ReconstructError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "Database error: {e}"),
            Self::NotFound => write!(f, "Revision not found"),
            Self::PatchParse(id, e) => write!(f, "Failed to parse patch for revision {id}: {e}"),
            Self::PatchApply(id, e) => write!(f, "Failed to apply patch for revision {id}: {e}"),
        }
    }
}
