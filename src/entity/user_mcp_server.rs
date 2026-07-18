use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "user_mcp_servers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_id: i32,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub forward_user_token: bool,
    pub headers: Json,
    /// Config-side MCP capability hint (#39, ADR-0117): raw (un-namespaced)
    /// remote tool name → capability (`read`/`write`/`call`,
    /// `crate::ai::tool_permissions::CAPABILITIES`), so a `tool_permissions`
    /// bare capability rule fans out to this server's matching
    /// `"{name}__{tool}"` identities alongside the built-in tools —
    /// `crate::ai::tool_permissions::mcp_capability_index`.
    pub capabilities: Json,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
