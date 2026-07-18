use sea_orm::entity::prelude::*;

/// Permission rule for an MCP / local tool (#39). `effect` is `"allow"`,
/// `"deny"`, or `"prompt"`. Rows build an `entanglement_core::PermissionProfile`
/// (`ai::tool_permissions::build_profile`); last-match-wins over rows ordered
/// `priority DESC, id DESC` reproduces the historical `priority` ASC (lower
/// runs first)/`id` first-match-wins semantics.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "tool_permissions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_id: i32,
    /// A literal tool name, the catch-all `*`, a capability key
    /// (`read`/`write`/`call`, `ai::tool_permissions::CAPABILITIES`), or a
    /// scoped form `tool(argpattern)` / `tool{workdirpattern}`.
    pub name: String,
    pub effect: String,
    pub priority: i32,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
