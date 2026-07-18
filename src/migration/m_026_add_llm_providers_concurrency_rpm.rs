use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_026_add_llm_providers_concurrency_rpm"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Nullable: an unset budget/cap falls back to entanglement_provider's
        // own `RetryConfig` defaults (ADR-0111) rather than refusing to save a
        // provider row, mirroring `m_025_add_llm_models_context_window`.
        manager
            .alter_table(
                Table::alter()
                    .table(LlmProviders::Table)
                    .add_column(ColumnDef::new(LlmProviders::Concurrency).integer().null())
                    .add_column(ColumnDef::new(LlmProviders::Rpm).integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(LlmProviders::Table)
                    .drop_column(LlmProviders::Concurrency)
                    .drop_column(LlmProviders::Rpm)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum LlmProviders {
    Table,
    Concurrency,
    Rpm,
}
