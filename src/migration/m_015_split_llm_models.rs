use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_015_split_llm_models"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. New table: llm_models { id, provider_id, label, model, is_default, created_at }
        manager
            .create_table(
                Table::create()
                    .table(LlmModels::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LlmModels::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LlmModels::ProviderId).integer().not_null())
                    .col(ColumnDef::new(LlmModels::Label).string().not_null())
                    .col(ColumnDef::new(LlmModels::Model).string().not_null())
                    .col(
                        ColumnDef::new(LlmModels::IsDefault)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(LlmModels::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_llm_models_provider")
                            .from(LlmModels::Table, LlmModels::ProviderId)
                            .to(LlmProviders::Table, LlmProviders::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 2. Backfill: one model row per existing provider, using the
        //    provider's `model` column. The first one becomes default.
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        conn.execute(sea_orm::Statement::from_string(
            backend,
            "INSERT INTO llm_models (provider_id, label, model, is_default, created_at) \
             SELECT id, label, model, is_default, created_at FROM llm_providers"
                .to_owned(),
        ))
        .await?;

        // 3. assistant_sessions.model_id (nullable FK)
        manager
            .alter_table(
                Table::alter()
                    .table(AssistantSessions::Table)
                    .add_column(ColumnDef::new(AssistantSessions::ModelId).integer().null())
                    .to_owned(),
            )
            .await?;

        // 4. Backfill model_id from provider_id (pick the matching model row)
        conn.execute(sea_orm::Statement::from_string(
            backend,
            "UPDATE assistant_sessions s \
             SET model_id = (SELECT id FROM llm_models m WHERE m.provider_id = s.provider_id LIMIT 1) \
             WHERE s.provider_id IS NOT NULL"
                .to_owned(),
        ))
        .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_assistant_sessions_model")
                    .from(AssistantSessions::Table, AssistantSessions::ModelId)
                    .to(LlmModels::Table, LlmModels::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .to_owned(),
            )
            .await?;

        // 5. Drop the now-redundant provider_id FK + column, and the
        //    `model` + `is_default` columns from llm_providers.
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
            .alter_table(
                Table::alter()
                    .table(LlmProviders::Table)
                    .drop_column(LlmProviders::Model)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(LlmProviders::Table)
                    .drop_column(LlmProviders::IsDefault)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // One-way migration; rolling back would risk losing model rows.
        Err(DbErr::Migration(
            "m_015_split_llm_models is not reversible".into(),
        ))
    }
}

#[derive(Iden)]
enum LlmModels {
    Table,
    Id,
    ProviderId,
    Label,
    Model,
    IsDefault,
    CreatedAt,
}

#[derive(Iden)]
enum LlmProviders {
    Table,
    Id,
    Model,
    IsDefault,
}

#[derive(Iden)]
enum AssistantSessions {
    Table,
    ProviderId,
    ModelId,
}
