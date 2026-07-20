use std::path::PathBuf;

pub struct Config {
    pub database_url: String,
    pub design_dir: Option<PathBuf>,
    pub serper_api_key: Option<String>,
    /// `pandoc` binary used by mdcast's DOCX/ODT/PPTX/reveal.js-slides
    /// backends (#64). No equivalent path exists for typst — it renders
    /// PDF/PDF-slides in-process via the `typst`/`typst-as-lib` crates.
    pub mdcast_pandoc_path: String,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            design_dir: std::env::var("DESIGN_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            serper_api_key: std::env::var("SERPER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            mdcast_pandoc_path: std::env::var("MDCAST_PANDOC_PATH")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "pandoc".to_string()),
        }
    }
}
