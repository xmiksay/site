//! Per-directive embed handlers, one family per file: `simple` (page/file/
//! image/gallery), `media` (fen/pgn/mermaid), `json` (jq-backed tables).

mod json;
mod media;
mod simple;

use sea_orm::DatabaseConnection;

use crate::files;

use super::directives::Directive;

pub(super) use json::directive_json;
pub(super) use media::{directive_fen, directive_mermaid, directive_pgn};
pub(super) use simple::{directive_file, directive_gallery, directive_img, directive_page};

// Re-exported at crate::markdown visibility solely so `tests.rs` can exercise
// them directly, matching the coverage the original single-file module had.
#[cfg(test)]
pub(super) use json::{json_table, run_jq};

/// Trimmed paired-tag body, if the directive carries a non-empty one.
pub(super) fn inline_body(d: &Directive) -> Option<String> {
    d.inner
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(super) fn parse_size_class(d: &Directive) -> &'static str {
    match d.arg("size").unwrap_or("") {
        "small" | "sm" => " size-sm",
        "large" | "lg" => " size-lg",
        _ => "",
    }
}

pub(super) async fn read_text_blob(db: &DatabaseConnection, hash: &str) -> Option<String> {
    let bytes = files::read_blob(db, hash).await.ok().flatten()?;
    String::from_utf8(bytes).ok()
}
