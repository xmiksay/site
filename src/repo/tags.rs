use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::entity::tag;

pub struct TagInput {
    pub name: String,
    pub description: Option<String>,
}

pub struct TagUpdate {
    pub new_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug)]
pub enum TagSaveError {
    EmptyName,
    Db(DbErr),
}

impl std::fmt::Display for TagSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "name is required"),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<DbErr> for TagSaveError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub async fn list_all(db: &DatabaseConnection) -> Result<Vec<tag::Model>, DbErr> {
    tag::Entity::find()
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
}

pub async fn create_tag(
    db: &DatabaseConnection,
    input: TagInput,
) -> Result<tag::Model, TagSaveError> {
    if input.name.trim().is_empty() {
        return Err(TagSaveError::EmptyName);
    }
    Ok(tag::ActiveModel {
        name: Set(input.name),
        description: Set(input.description.filter(|s| !s.is_empty())),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn find_by_name(
    db: &DatabaseConnection,
    name: &str,
) -> Result<Option<tag::Model>, DbErr> {
    tag::Entity::find()
        .filter(tag::Column::Name.eq(name))
        .one(db)
        .await
}

pub async fn update_tag_by_id(
    db: &DatabaseConnection,
    id: i32,
    name: String,
    description: Option<String>,
) -> Result<Option<tag::Model>, DbErr> {
    let Some(model) = tag::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let mut active: tag::ActiveModel = model.into();
    active.name = Set(name);
    active.description = Set(description.filter(|s| !s.is_empty()));
    Ok(Some(active.update(db).await?))
}

pub async fn update_tag_by_name(
    db: &DatabaseConnection,
    name: &str,
    update: TagUpdate,
) -> Result<Option<tag::Model>, DbErr> {
    let Some(model) = find_by_name(db, name).await? else {
        return Ok(None);
    };
    let mut active: tag::ActiveModel = model.into();
    if let Some(n) = update.new_name {
        active.name = Set(n);
    }
    if let Some(d) = update.description {
        active.description = Set(Some(d).filter(|s| !s.is_empty()));
    }
    Ok(Some(active.update(db).await?))
}

/// Delete a tag by name, returning its id (for a WS broadcast) if it existed.
pub async fn delete_by_name(db: &DatabaseConnection, name: &str) -> Result<Option<i32>, DbErr> {
    let Some(model) = find_by_name(db, name).await? else {
        return Ok(None);
    };
    tag::Entity::delete_by_id(model.id).exec(db).await?;
    Ok(Some(model.id))
}

/// Outcome of resolving tag names to IDs: the IDs that matched existing tags,
/// plus any requested names that had no matching tag (skipped, not an error).
#[derive(Debug)]
pub struct ResolvedTags {
    pub ids: Vec<i32>,
    pub missing: Vec<String>,
}

/// Human-readable suffix listing skipped tag names, or "" when none were skipped.
pub fn skipped_note(missing: &[String]) -> String {
    if missing.is_empty() {
        String::new()
    } else {
        format!(" (skipped unknown tags: {})", missing.join(", "))
    }
}

/// Look up tag IDs by name. Names with no matching tag are skipped and returned
/// in `missing` rather than failing the whole call — a single unknown tag must
/// not block a page write.
pub async fn resolve_ids(db: &DatabaseConnection, names: &[String]) -> Result<ResolvedTags, DbErr> {
    if names.is_empty() {
        return Ok(ResolvedTags {
            ids: vec![],
            missing: vec![],
        });
    }
    let tags = tag::Entity::find()
        .filter(tag::Column::Name.is_in(names.iter().map(|s| s.as_str())))
        .all(db)
        .await?;
    let found: Vec<String> = tags.iter().map(|t| t.name.clone()).collect();
    let missing: Vec<String> = names
        .iter()
        .filter(|n| !found.contains(n))
        .cloned()
        .collect();
    Ok(ResolvedTags {
        ids: tags.iter().map(|t| t.id).collect(),
        missing,
    })
}

/// Best-effort resolution of tag names by id. Missing ids are silently skipped.
pub async fn resolve_names(db: &DatabaseConnection, ids: &[i32]) -> Vec<String> {
    if ids.is_empty() {
        return vec![];
    }
    tag::Entity::find()
        .filter(tag::Column::Id.is_in(ids.iter().copied()))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::skipped_note;

    #[test]
    fn skipped_note_empty_is_blank() {
        assert_eq!(skipped_note(&[]), "");
    }

    #[test]
    fn skipped_note_lists_names() {
        let note = skipped_note(&["foo".to_string(), "bar".to_string()]);
        assert_eq!(note, " (skipped unknown tags: foo, bar)");
    }
}
