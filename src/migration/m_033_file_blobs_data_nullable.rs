use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_033_file_blobs_data_nullable"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // #109: with STORAGE_KIND=fs|s3 the bytes live outside Postgres and a
        // `file_blobs` row keeps only hash + size (the FK target for
        // `files`/`file_thumbnails`).
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE file_blobs ALTER COLUMN data DROP NOT NULL")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Fails while metadata-only rows exist — on purpose: those blobs' bytes
        // are in fs/s3 and must be migrated back (`storage migrate` with
        // STORAGE_KIND=db) before the column can be required again.
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE file_blobs ALTER COLUMN data SET NOT NULL")
            .await?;
        Ok(())
    }
}
