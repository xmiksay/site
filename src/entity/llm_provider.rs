use sea_orm::entity::prelude::*;

/// A connection to an LLM provider (kind + credentials). Models live in a
/// separate table — one provider row can host many models.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "llm_providers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Display name shown in the UI (unique).
    pub label: String,
    /// `anthropic`, `ollama`, or `gemini`.
    pub kind: String,
    /// API key for `anthropic` / `gemini`.
    #[sea_orm(nullable)]
    pub api_key: Option<String>,
    /// Base URL for `ollama`.
    #[sea_orm(nullable)]
    pub base_url: Option<String>,
    /// Max simultaneously in-flight requests to this provider's endpoint
    /// (ADR-0111 per-endpoint concurrency permit, held across a whole
    /// streamed turn). `None` falls back to `entanglement_provider`'s own
    /// client default.
    #[sea_orm(nullable)]
    pub concurrency: Option<i32>,
    /// Requests-per-minute budget for this provider's endpoint. `None` falls
    /// back to `entanglement_provider`'s own client default.
    #[sea_orm(nullable)]
    pub rpm: Option<i32>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::llm_model::Entity")]
    Models,
}

impl Related<super::llm_model::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Models.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
