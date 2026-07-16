pub use sea_orm_migration::prelude::*;

mod m_001_create_users;
mod m_002_create_tokens;
mod m_003_create_tags;
mod m_004_create_menus;
mod m_005_create_pages;
mod m_006_create_images;
mod m_007_create_galleries;
mod m_008_add_menu_private;
mod m_009_create_oauth;
mod m_010_replace_images_with_files;
mod m_011_create_assistant_sessions;
mod m_012_create_assistant_messages;
mod m_013_create_user_mcp_servers;
mod m_014_create_llm_providers;
mod m_015_split_llm_models;
mod m_016_create_tool_permissions;
mod m_017_add_files_path;
mod m_018_add_assistant_sessions_mcp_server_ids;
mod m_019_drop_assistant_sessions_system_prompt;
mod m_020_add_galleries_path;
mod m_021_normalize_paths;
mod m_022_add_pages_fulltext;
mod m_023_create_assistant_events;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m_001_create_users::Migration),
            Box::new(m_002_create_tokens::Migration),
            Box::new(m_003_create_tags::Migration),
            Box::new(m_004_create_menus::Migration),
            Box::new(m_005_create_pages::Migration),
            Box::new(m_006_create_images::Migration),
            Box::new(m_007_create_galleries::Migration),
            Box::new(m_008_add_menu_private::Migration),
            Box::new(m_009_create_oauth::Migration),
            Box::new(m_010_replace_images_with_files::Migration),
            Box::new(m_011_create_assistant_sessions::Migration),
            Box::new(m_012_create_assistant_messages::Migration),
            Box::new(m_013_create_user_mcp_servers::Migration),
            Box::new(m_014_create_llm_providers::Migration),
            Box::new(m_015_split_llm_models::Migration),
            Box::new(m_016_create_tool_permissions::Migration),
            Box::new(m_017_add_files_path::Migration),
            Box::new(m_018_add_assistant_sessions_mcp_server_ids::Migration),
            Box::new(m_019_drop_assistant_sessions_system_prompt::Migration),
            Box::new(m_020_add_galleries_path::Migration),
            Box::new(m_021_normalize_paths::Migration),
            Box::new(m_022_add_pages_fulltext::Migration),
            Box::new(m_023_create_assistant_events::Migration),
        ]
    }
}
