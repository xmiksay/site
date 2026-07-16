use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_022_add_pages_fulltext"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("CREATE EXTENSION IF NOT EXISTS unaccent")
            .await?;
        // unaccent() is STABLE; wrap as IMMUTABLE so it can be used in a generated column / index.
        db.execute_unprepared(
            "CREATE OR REPLACE FUNCTION public.f_unaccent(text) RETURNS text \
             AS $$ SELECT public.unaccent('public.unaccent', $1) $$ \
             LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE pages ADD COLUMN search_tsv tsvector GENERATED ALWAYS AS ( \
                setweight(to_tsvector('simple', f_unaccent(path)), 'A') || \
                setweight(to_tsvector('simple', f_unaccent(coalesce(summary, ''))), 'B') || \
                setweight(to_tsvector('simple', f_unaccent(markdown)), 'C') \
             ) STORED",
        )
        .await?;
        db.execute_unprepared("CREATE INDEX pages_search_tsv_idx ON pages USING GIN (search_tsv)")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS pages_search_tsv_idx")
            .await?;
        db.execute_unprepared("ALTER TABLE pages DROP COLUMN IF EXISTS search_tsv")
            .await?;
        db.execute_unprepared("DROP FUNCTION IF EXISTS public.f_unaccent(text)")
            .await?;
        Ok(())
    }
}
