use sea_orm::entity::prelude::*;

/// One row per `entanglement_runtime::session_store::LogRecord`, appended by
/// `crate::ai::persistence`'s `RecordSink` impl. Joined to a session at the
/// application level by `root_session_id` (the engine's `SessionId` string,
/// `u{user_id}:{uuid}` convention) — not a DB foreign key, since the engine
/// has no notion of `assistant_sessions.id`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
#[sea_orm(table_name = "assistant_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub root_session_id: String,
    pub payload: Json,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
