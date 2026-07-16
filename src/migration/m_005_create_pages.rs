use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_005_create_pages"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Pages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Pages::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Pages::Path).string().not_null().unique_key())
                    .col(ColumnDef::new(Pages::Summary).text().null())
                    .col(ColumnDef::new(Pages::Markdown).text().not_null())
                    .col(
                        ColumnDef::new(Pages::TagIds)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("'{}'::int[]")),
                    )
                    .col(
                        ColumnDef::new(Pages::Private)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Pages::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Pages::CreatedBy).integer().not_null())
                    .col(
                        ColumnDef::new(Pages::ModifiedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Pages::ModifiedBy).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(Pages::Table, Pages::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Pages::Table, Pages::ModifiedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PageRevisions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PageRevisions::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PageRevisions::PageId).integer().not_null())
                    .col(ColumnDef::new(PageRevisions::Patch).text().not_null())
                    .col(
                        ColumnDef::new(PageRevisions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(PageRevisions::CreatedBy)
                            .integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(PageRevisions::Table, PageRevisions::PageId)
                            .to(Pages::Table, Pages::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(PageRevisions::Table, PageRevisions::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PageRevisions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Pages::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Pages {
    Table,
    Id,
    Path,
    Summary,
    Markdown,
    TagIds,
    Private,
    CreatedAt,
    CreatedBy,
    ModifiedAt,
    ModifiedBy,
}

#[derive(Iden)]
enum PageRevisions {
    Table,
    Id,
    PageId,
    Patch,
    CreatedAt,
    CreatedBy,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
