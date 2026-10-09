use std::path::PathBuf;

use anyhow::Context as _;

use crate::storage::StorageConfig;

pub struct Config {
    pub database_url: String,
    pub design_dir: Option<PathBuf>,
    pub serper_api_key: Option<String>,
    /// Base URL of the remote `mdcast-server` that renders PDF/slides
    /// exports. Unset → export routes answer 503; nothing else degrades.
    pub mdcast_url: Option<String>,
    /// Bearer token for `mdcast-server`. Unset is fine against a tokenless
    /// server — the client requires *some* token, so a placeholder is
    /// substituted at client construction (`export::build_client`).
    pub mdcast_token: Option<String>,
    /// Where file and thumbnail bytes live (`STORAGE_KIND`, default `db`).
    pub storage: StorageConfig,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            design_dir: std::env::var("DESIGN_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            serper_api_key: std::env::var("SERPER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            mdcast_url: std::env::var("MDCAST_URL").ok().filter(|s| !s.is_empty()),
            mdcast_token: std::env::var("MDCAST_TOKEN").ok().filter(|s| !s.is_empty()),
            storage: StorageConfig::from_env()?,
        })
    }
}
