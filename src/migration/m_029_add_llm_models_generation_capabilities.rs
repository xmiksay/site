use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_029_add_llm_models_generation_capabilities"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Gate the generation knobs the site's UI exposes (#53): forwarding
        // e.g. `reasoning_effort` to a model that doesn't accept it makes the
        // provider reject the whole turn with a 400. `supports_temperature`
        // defaults `true` (most models accept it; least likely to regress
        // existing rows), the other two default `false` (opt-in, matching
        // the bug report's exact knob).
        manager
            .alter_table(
                Table::alter()
                    .table(LlmModels::Table)
                    .add_column(
                        ColumnDef::new(LlmModels::SupportsTemperature)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .add_column(
                        ColumnDef::new(LlmModels::SupportsReasoningEffort)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(LlmModels::SupportsThinking)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(LlmModels::Table)
                    .drop_column(LlmModels::SupportsTemperature)
                    .drop_column(LlmModels::SupportsReasoningEffort)
                    .drop_column(LlmModels::SupportsThinking)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum LlmModels {
    Table,
    SupportsTemperature,
    SupportsReasoningEffort,
    SupportsThinking,
}
