use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_035_add_llm_models_supports_images"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // #132: `file_read`/`design_read` hand the model an image block only
        // when it can see; a text-only model would reject the whole turn.
        // Defaults `true` — current mainstream chat models are multimodal.
        manager
            .alter_table(
                Table::alter()
                    .table(LlmModels::Table)
                    .add_column(
                        ColumnDef::new(LlmModels::SupportsImages)
                            .boolean()
                            .not_null()
                            .default(true),
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
                    .drop_column(LlmModels::SupportsImages)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum LlmModels {
    Table,
    SupportsImages,
}
