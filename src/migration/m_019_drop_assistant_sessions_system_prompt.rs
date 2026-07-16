use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_019_drop_assistant_sessions_system_prompt"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .drop_column(AssistantSessions::SystemPrompt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(
                        ColumnDef::new(AssistantSessions::SystemPrompt)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    SystemPrompt,
}
