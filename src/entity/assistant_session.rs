use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "assistant_sessions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_id: i32,
    pub title: String,
    /// Snapshot of the provider kind at session creation (kept for display).
    pub provider: String,
    /// Snapshot of the model wire identifier at session creation.
    pub model: String,
    /// FK into `llm_models` — the live model row used to dispatch chat
    /// requests. Nullable so the session survives if the model is removed.
    #[sea_orm(nullable)]
    pub model_id: Option<i32>,
    /// IDs of `user_mcp_servers` rows whose tools are exposed to this session.
    /// Stored as JSONB array of integers; empty means no MCP tools.
    pub enabled_mcp_server_ids: Json,
    /// The engine's root `SessionId` string (`u{user_id}:{uuid}` convention) —
    /// this DB row's 1:1 link to its `assistant_events` log. Nullable because
    /// rows created before the engine swap never get one back; unique because
    /// a DB session row owns exactly one engine session (m_023).
    #[sea_orm(nullable, unique)]
    pub engine_session_id: Option<String>,
    /// Session-level generation overrides (#42, ADR-0094), mirrored onto the
    /// live engine session via `InMsg::SetGeneration` — `None` leaves that knob
    /// at the model's own default (m_027).
    #[sea_orm(nullable)]
    pub temperature: Option<f32>,
    /// `"low" | "medium" | "high"` — `entanglement_provider::ReasoningEffort`'s
    /// wire representation (m_027).
    #[sea_orm(nullable)]
    pub reasoning_effort: Option<String>,
    /// The engine agent profile this session runs under (`build` |
    /// `researcher` | `page-writer`, `src/ai/engine/profiles.rs`), mirrored via
    /// `InMsg::SetAgent` (m_027).
    pub agent_profile: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::llm_model::Entity",
        from = "Column::ModelId",
        to = "super::llm_model::Column::Id",
        on_delete = "SetNull"
    )]
    LlmModel,
}

impl Related<super::llm_model::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LlmModel.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
