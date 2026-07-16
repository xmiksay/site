use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_023_create_assistant_events"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The engine (entanglement) replaces the old per-message chat log with
        // an event-sourced log: one `assistant_events` row per `LogRecord`,
        // joined to a session by the engine's own `SessionId` string
        // (`root_session_id`), not a DB foreign key — the engine has no
        // concept of our `assistant_sessions.id`.
        manager
            .create_table(
                Table::create()
                    .table(AssistantEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AssistantEvents::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AssistantEvents::RootSessionId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AssistantEvents::Payload)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AssistantEvents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_assistant_events_root_session_id")
                    .table(AssistantEvents::Table)
                    .col(AssistantEvents::RootSessionId)
                    .to_owned(),
            )
            .await?;

        // 1:1 link from a DB session row to the engine's root `SessionId`.
        // Nullable: existing rows predate the engine and never get one back
        // (no conversion path from the old message format — see below).
        // Unique: a DB session row owns exactly one engine session.
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(
                        ColumnDef::new(AssistantSessions::EngineSessionId)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_assistant_sessions_engine_session_id")
                    .table(AssistantSessions::Table)
                    .col(AssistantSessions::EngineSessionId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Settled decision: no conversion path from the old per-message chat
        // format to the engine's event log, so existing sessions are wiped
        // rather than migrated. `assistant_messages` cascades from
        // `assistant_sessions` on delete, but drop the table outright anyway
        // (it's being replaced by `assistant_events`, not just emptied).
        let db = manager.get_connection();
        db.execute_unprepared("DELETE FROM assistant_sessions")
            .await?;
        manager
            .drop_table(Table::drop().table(AssistantMessages::Table).to_owned())
            .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Lossy, irreversible: existing assistant_sessions rows were deleted
        // and assistant_messages was dropped in `up()`. Same precedent as
        // `m_015_split_llm_models`.
        Err(DbErr::Migration(
            "m_023_create_assistant_events is not reversible (data deleted, table dropped)".into(),
        ))
    }
}

#[derive(Iden)]
enum AssistantEvents {
    Table,
    Id,
    RootSessionId,
    Payload,
    CreatedAt,
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    EngineSessionId,
}

#[derive(Iden)]
enum AssistantMessages {
    Table,
}
