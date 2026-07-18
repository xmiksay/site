use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_027_add_assistant_sessions_generation"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `temperature`/`reasoning_effort` are nullable partial overrides (#42,
        // ADR-0094) — `None` means "no session-level override", matching
        // `entanglement_provider::GenerationParams`'s own "leave unchanged"
        // semantics rather than needing a sentinel. `agent_profile` defaults to
        // the engine's built-in root profile name (`"build"`,
        // `src/ai/engine/profiles.rs`) so every pre-existing row keeps running
        // under the profile it already started under.
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(
                        ColumnDef::new(AssistantSessions::Temperature)
                            .float()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(AssistantSessions::ReasoningEffort)
                            .string()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(AssistantSessions::AgentProfile)
                            .string()
                            .not_null()
                            .default("build"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .drop_column(AssistantSessions::Temperature)
                    .drop_column(AssistantSessions::ReasoningEffort)
                    .drop_column(AssistantSessions::AgentProfile)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    Temperature,
    ReasoningEffort,
    AgentProfile,
}
