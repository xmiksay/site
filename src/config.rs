use std::path::PathBuf;

pub struct Config {
    pub database_url: String,
    pub namespace: String,
    pub assets_dir: Option<PathBuf>,
    pub serper_api_key: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            namespace: std::env::var("NAMESPACE").unwrap_or_else(|_| "common".into()),
            assets_dir: std::env::var("ASSETS_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            serper_api_key: std::env::var("SERPER_API_KEY").ok().filter(|s| !s.is_empty()),
        }
    }
}
