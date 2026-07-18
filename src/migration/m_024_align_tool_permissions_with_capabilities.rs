use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_024_align_tool_permissions_with_capabilities"
    }
}

/// This site's current built-in (non-MCP) tool vocabulary
/// (`src/ai/tools/mod.rs::registry`) — used only to deterministically expand
/// old-style name-prefix-wildcard `tool_permissions` rows below, a snapshot
/// frozen at migration-authoring time, not a live lookup.
const BUILTIN_TOOLS: &[&str] = &[
    "read_page",
    "search_pages",
    "edit_page",
    "delete_page",
    "list_tags",
    "create_tag",
    "list_files",
    "create_file",
    "list_galleries",
    "create_gallery",
    "update_gallery",
    "web_search",
    "web_fetch",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // #39: config-side MCP capability hint (ADR-0117) — raw remote tool
        // name → capability (`read`/`write`/`call`), mirroring
        // `entanglement_runtime::mcp::McpServerConfig.capabilities` but keyed
        // to this site's own `"{server}__{tool}"` naming (`ai::mcp::SiteMcp`).
        manager
            .alter_table(
                Table::alter()
                    .table(UserMcpServers::Table)
                    .add_column(
                        ColumnDef::new(UserMcpServers::Capabilities)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .to_owned(),
            )
            .await?;

        // #39: the bespoke `*`-suffix prefix matcher (`tool_permissions.rs`'s
        // old `matches()`) is retired in favor of
        // `entanglement_core::PermissionProfile::resolve_scoped`, whose rule
        // keys match a tool *name* exactly (or the literal `*` for "every
        // tool") — a name-prefix glob like `edit_*` has no equivalent there.
        // The bare `*` catch-all needs no translation (still matches every
        // tool under the new resolver). For every other `prefix*` row, expand
        // it into one literal-name row per currently-known built-in tool the
        // prefix matched — deterministic only against `BUILTIN_TOOLS`, since
        // an MCP server's remote tool names aren't knowable from a migration.
        for tool in BUILTIN_TOOLS {
            let sql = format!(
                "INSERT INTO tool_permissions (user_id, name, effect, priority, created_at) \
                 SELECT user_id, '{tool}', effect, priority, created_at FROM tool_permissions \
                 WHERE name <> '*' AND name LIKE '%*' \
                   AND '{tool}' LIKE left(name, length(name) - 1) || '%'"
            );
            db.execute_unprepared(&sql).await?;
        }
        db.execute_unprepared(
            "DELETE FROM tool_permissions tp WHERE tp.name <> '*' AND tp.name LIKE '%*' \
             AND EXISTS ( \
               SELECT 1 FROM unnest(ARRAY[\
                 'read_page','search_pages','edit_page','delete_page','list_tags','create_tag',\
                 'list_files','create_file','list_galleries','create_gallery','update_gallery',\
                 'web_search','web_fetch'\
               ]) AS t(tool_name) \
               WHERE t.tool_name LIKE left(tp.name, length(tp.name) - 1) || '%' \
             )",
        )
        .await?;

        // A `prefix*` row that matched none of the built-in tools above is
        // presumably an old MCP-server-name prefix (e.g. `blog__*`) — left
        // untouched (so it now simply never matches anything, fail-closed
        // rather than fail-open) but flagged so an admin re-authors it as a
        // capability or scoped rule via the admin UI.
        db.execute_unprepared(
            "DO $$ DECLARE cnt integer; BEGIN \
               SELECT count(*) INTO cnt FROM tool_permissions WHERE name <> '*' AND name LIKE '%*'; \
               IF cnt > 0 THEN \
                 RAISE NOTICE '% tool_permissions row(s) use a name-prefix wildcard this \
                   migration could not translate (likely an old MCP server prefix) — they no \
                   longer match anything; recreate them as capability or scoped rules via the \
                   admin UI', cnt; \
               END IF; \
             END $$;",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The `tool_permissions` expansion above is lossy (the original
        // wildcard row is deleted once expanded) — not reversed here, same as
        // this repo's other data migrations (e.g. m_021). Only the schema
        // change rolls back.
        manager
            .alter_table(
                Table::alter()
                    .table(UserMcpServers::Table)
                    .drop_column(UserMcpServers::Capabilities)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum UserMcpServers {
    Table,
    Capabilities,
}
