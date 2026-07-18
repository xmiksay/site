use sea_orm::entity::prelude::*;

/// A model belonging to an `llm_providers` row. One provider connection can
/// expose many models (e.g. Anthropic with `claude-opus-4-1`, `claude-sonnet-4-5`,
/// `claude-haiku-4-5` all share the same api_key).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "llm_models")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub provider_id: i32,
    /// Display label shown in the UI (e.g. "Claude Sonnet 4.5").
    pub label: String,
    /// Wire identifier sent to the provider (e.g. "claude-sonnet-4-5-20250929").
    pub model: String,
    /// First-created model is auto-flagged as default for new sessions.
    pub is_default: bool,
    /// The model's real context window in tokens (#40), fed to
    /// `entanglement_provider::ResolvedModel::context_window` so the engine
    /// compacts/refuses against the actual budget instead of a generic
    /// fallback. `None` when unset (falls back to
    /// `entanglement_core::context::CONTEXT_LIMIT_TOKENS`).
    pub context_window: Option<i32>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::llm_provider::Entity",
        from = "Column::ProviderId",
        to = "super::llm_provider::Column::Id",
        on_delete = "Cascade"
    )]
    Provider,
}

impl Related<super::llm_provider::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provider.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
