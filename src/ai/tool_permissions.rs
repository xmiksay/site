use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entity::tool_permission;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Deny,
    Prompt,
}

impl Effect {
    #[allow(clippy::should_implement_trait)] // deliberately infallible, unlike `FromStr::from_str`
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "allow" => Effect::Allow,
            "deny" => Effect::Deny,
            _ => Effect::Prompt,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Allow => "allow",
            Effect::Deny => "deny",
            Effect::Prompt => "prompt",
        }
    }
}

/// Resolve the effect for a tool call. First-match wins, ordered by
/// `priority ASC, id ASC`. If nothing matches, default is `Prompt` — the user
/// approves explicitly. Names ending in `*` are treated as a prefix wildcard.
pub async fn resolve(
    db: &DatabaseConnection,
    user_id: i32,
    tool_name: &str,
) -> anyhow::Result<Effect> {
    let rules = tool_permission::Entity::find()
        .filter(tool_permission::Column::UserId.eq(user_id))
        .order_by_asc(tool_permission::Column::Priority)
        .order_by_asc(tool_permission::Column::Id)
        .all(db)
        .await?;

    for rule in &rules {
        if matches(&rule.name, tool_name) {
            return Ok(Effect::from_str(&rule.effect));
        }
    }
    Ok(Effect::Prompt)
}

fn matches(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        pattern == name
    }
}
