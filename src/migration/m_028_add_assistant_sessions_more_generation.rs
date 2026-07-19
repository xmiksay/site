use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_028_add_assistant_sessions_more_generation"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `max_output_tokens`/`thinking_budget_tokens` are the other two
        // fields of `entanglement_provider::GenerationParams` (m_027 already
        // covers `temperature`/`reasoning_effort`) — nullable session-level
        // overrides, `None` meaning "no override, use the model's default".
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(
                        ColumnDef::new(AssistantSessions::MaxOutputTokens)
                            .integer()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(AssistantSessions::ThinkingBudgetTokens)
                            .integer()
                            .null(),
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
                    .drop_column(AssistantSessions::MaxOutputTokens)
                    .drop_column(AssistantSessions::ThinkingBudgetTokens)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    MaxOutputTokens,
    ThinkingBudgetTokens,
}
