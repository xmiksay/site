use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_010_replace_images_with_files"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Galleries::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Images::Table).if_exists().to_owned())
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FileBlobs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileBlobs::Hash)
                            .char_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FileBlobs::Data).binary().not_null())
                    .col(
                        ColumnDef::new(FileBlobs::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FileBlobs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Files::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Files::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Files::Hash).char_len(64).not_null())
                    .col(ColumnDef::new(Files::Mimetype).string().not_null())
                    .col(ColumnDef::new(Files::Title).string().not_null())
                    .col(ColumnDef::new(Files::Description).text().null())
                    .col(ColumnDef::new(Files::SizeBytes).big_integer().not_null())
                    .col(
                        ColumnDef::new(Files::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Files::CreatedBy).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_files_hash")
                            .from(Files::Table, Files::Hash)
                            .to(FileBlobs::Table, FileBlobs::Hash)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_files_created_by")
                            .from(Files::Table, Files::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_files_hash")
                    .table(Files::Table)
                    .col(Files::Hash)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FileThumbnails::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileThumbnails::FileId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FileThumbnails::Hash).char_len(64).not_null())
                    .col(ColumnDef::new(FileThumbnails::Width).integer().not_null())
                    .col(ColumnDef::new(FileThumbnails::Height).integer().not_null())
                    .col(ColumnDef::new(FileThumbnails::Mimetype).string().not_null())
                    .col(
                        ColumnDef::new(FileThumbnails::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_file_thumbnails_file_id")
                            .from(FileThumbnails::Table, FileThumbnails::FileId)
                            .to(Files::Table, Files::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_file_thumbnails_hash")
                            .from(FileThumbnails::Table, FileThumbnails::Hash)
                            .to(FileBlobs::Table, FileBlobs::Hash)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Galleries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Galleries::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Galleries::Title).string().not_null())
                    .col(ColumnDef::new(Galleries::Description).text().null())
                    .col(
                        ColumnDef::new(Galleries::FileIds)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("'{}'::int[]")),
                    )
                    .col(
                        ColumnDef::new(Galleries::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Galleries::CreatedBy).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_galleries_created_by")
                            .from(Galleries::Table, Galleries::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Galleries::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(FileThumbnails::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Files::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(FileBlobs::Table).if_exists().to_owned())
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Images::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Images::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Images::Title).string().not_null())
                    .col(ColumnDef::new(Images::Description).text().null())
                    .col(ColumnDef::new(Images::Data).binary().not_null())
                    .col(ColumnDef::new(Images::Thumbnail).binary().not_null())
                    .col(
                        ColumnDef::new(Images::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Images::CreatedBy).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(Images::Table, Images::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Galleries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Galleries::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Galleries::Title).string().not_null())
                    .col(ColumnDef::new(Galleries::Description).text().null())
                    .col(
                        ColumnDef::new(Galleries::ImageIds)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("'{}'::int[]")),
                    )
                    .col(
                        ColumnDef::new(Galleries::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Galleries::CreatedBy).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(Galleries::Table, Galleries::CreatedBy)
                            .to(Users::Table, Users::Id),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum FileBlobs {
    Table,
    Hash,
    Data,
    SizeBytes,
    CreatedAt,
}

#[derive(Iden)]
enum Files {
    Table,
    Id,
    Hash,
    Mimetype,
    Title,
    Description,
    SizeBytes,
    CreatedAt,
    CreatedBy,
}

#[derive(Iden)]
enum FileThumbnails {
    Table,
    FileId,
    Hash,
    Width,
    Height,
    Mimetype,
    CreatedAt,
}

#[derive(Iden)]
enum Galleries {
    Table,
    Id,
    Title,
    Description,
    ImageIds,
    FileIds,
    CreatedAt,
    CreatedBy,
}

#[derive(Iden)]
enum Images {
    Table,
    Id,
    Title,
    Description,
    Data,
    Thumbnail,
    CreatedAt,
    CreatedBy,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
