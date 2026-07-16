use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_014_create_llm_providers"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LlmProviders::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LlmProviders::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(LlmProviders::Label)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(LlmProviders::Kind).string().not_null())
                    .col(ColumnDef::new(LlmProviders::Model).string().not_null())
                    .col(ColumnDef::new(LlmProviders::ApiKey).string().null())
                    .col(ColumnDef::new(LlmProviders::BaseUrl).string().null())
                    .col(
                        ColumnDef::new(LlmProviders::IsDefault)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(LlmProviders::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // Sessions now reference a provider row by id (nullable for legacy rows).
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(
                        ColumnDef::new(AssistantSessions::ProviderId)
                            .integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_assistant_sessions_provider")
                    .from(AssistantSessions::Table, AssistantSessions::ProviderId)
                    .to(LlmProviders::Table, LlmProviders::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk_assistant_sessions_provider")
                    .table(AssistantSessions::Table)
                    .to_owned(),
            )
            .await
            .ok();
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .drop_column(AssistantSessions::ProviderId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(LlmProviders::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum LlmProviders {
    Table,
    Id,
    Label,
    Kind,
    Model,
    ApiKey,
    BaseUrl,
    IsDefault,
    CreatedAt,
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    ProviderId,
}
