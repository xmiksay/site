use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_004_create_menus"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Menus::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Menus::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Menus::Title).string().not_null())
                    .col(ColumnDef::new(Menus::Path).string().not_null().unique_key())
                    .col(ColumnDef::new(Menus::Markdown).text().not_null())
                    .col(
                        ColumnDef::new(Menus::OrderIndex)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Menus::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Menus {
    Table,
    Id,
    Title,
    Path,
    Markdown,
    OrderIndex,
}
