use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_030_rename_tools_to_resource_operation"
    }
}

/// Old `<operation>_<resource>` -> new `<resource>_<operation>` tool name
/// mapping (issue #61). `web_search`/`web_fetch` are already
/// resource-first and are intentionally excluded.
const RENAMES: &[(&str, &str)] = &[
    ("read_page", "page_read"),
    ("search_pages", "page_search"),
    ("edit_page", "page_edit"),
    ("delete_page", "page_delete"),
    ("list_files", "file_list"),
    ("read_file", "file_read"),
    ("create_file", "file_create"),
    ("update_file", "file_update"),
    ("delete_file", "file_delete"),
    ("list_galleries", "gallery_list"),
    ("read_gallery", "gallery_read"),
    ("create_gallery", "gallery_create"),
    ("update_gallery", "gallery_update"),
    ("delete_gallery", "gallery_delete"),
    ("list_tags", "tag_list"),
    ("read_tag", "tag_read"),
    ("create_tag", "tag_create"),
    ("update_tag", "tag_update"),
    ("delete_tag", "tag_delete"),
];

/// Rewrite every `tool_permissions.name` row for `from` -> `to`, covering the
/// three shapes `PermissionProfile::resolve_scoped` (via
/// `ai::tool_permissions::split_rule_key`) can produce for a rule key: an
/// exact literal (`'read_page'`), an arg-scoped rule (`'read_page(...)'`),
/// and a workdir-scoped rule (`'read_page{...}'`). The bare `*` catch-all and
/// the `read`/`write`/`call` capability keys never match `from`/`to` here, so
/// they pass through untouched.
async fn rename(db: &SchemaManagerConnection<'_>, from: &str, to: &str) -> Result<(), DbErr> {
    let sql = format!(
        "UPDATE tool_permissions SET name = '{to}' || substring(name from length('{from}') + 1) \
         WHERE name = '{from}' OR name LIKE '{from}(%' OR name LIKE '{from}{{%'"
    );
    db.execute_unprepared(&sql).await?;
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (from, to) in RENAMES {
            rename(db, from, to).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Unlike m_024's lossy wildcard expansion, this rename is exact and
        // symmetric — reverse it with the same per-pair UPDATE, swapped.
        let db = manager.get_connection();
        for (from, to) in RENAMES {
            rename(db, to, from).await?;
        }
        Ok(())
    }
}
