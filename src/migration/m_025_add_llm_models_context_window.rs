use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_025_add_llm_models_context_window"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Nullable: an unset window falls back to the engine's generic
        // default budget (`entanglement_core::context::CONTEXT_LIMIT_TOKENS`)
        // rather than refusing to save a model row (#40).
        manager
            .alter_table(
                Table::alter()
                    .table(LlmModels::Table)
                    .add_column(ColumnDef::new(LlmModels::ContextWindow).integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(LlmModels::Table)
                    .drop_column(LlmModels::ContextWindow)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum LlmModels {
    Table,
    ContextWindow,
}
