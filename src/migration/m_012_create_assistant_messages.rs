use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_012_create_assistant_messages"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AssistantMessages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AssistantMessages::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AssistantMessages::SessionId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AssistantMessages::Seq).integer().not_null())
                    .col(ColumnDef::new(AssistantMessages::Role).string().not_null())
                    .col(
                        ColumnDef::new(AssistantMessages::Content)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AssistantMessages::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AssistantMessages::Table, AssistantMessages::SessionId)
                            .to(AssistantSessions::Table, AssistantSessions::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_assistant_messages_session_seq")
                    .table(AssistantMessages::Table)
                    .col(AssistantMessages::SessionId)
                    .col(AssistantMessages::Seq)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AssistantMessages::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum AssistantMessages {
    Table,
    Id,
    SessionId,
    Seq,
    Role,
    Content,
    CreatedAt,
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    Id,
}
