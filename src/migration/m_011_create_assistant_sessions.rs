use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_011_create_assistant_sessions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AssistantSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AssistantSessions::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AssistantSessions::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AssistantSessions::Title).string().not_null())
                    .col(
                        ColumnDef::new(AssistantSessions::SystemPrompt)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AssistantSessions::Provider)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AssistantSessions::Model).string().not_null())
                    .col(
                        ColumnDef::new(AssistantSessions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AssistantSessions::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AssistantSessions::Table, AssistantSessions::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_assistant_sessions_user_id")
                    .table(AssistantSessions::Table)
                    .col(AssistantSessions::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AssistantSessions::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    Id,
    UserId,
    Title,
    SystemPrompt,
    Provider,
    Model,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
