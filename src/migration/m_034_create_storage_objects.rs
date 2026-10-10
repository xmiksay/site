use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_034_create_storage_objects"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // #114: keyed objects (design overrides, …) for STORAGE_KIND=db, the
        // counterpart of the `fs`/`s3` object keys. `etag` is the sha256 of
        // `data`.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS storage_objects (
                    key TEXT PRIMARY KEY,
                    data BYTEA NOT NULL,
                    etag TEXT NOT NULL,
                    updated_at TIMESTAMPTZ NOT NULL
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS storage_objects")
            .await?;
        Ok(())
    }
}
