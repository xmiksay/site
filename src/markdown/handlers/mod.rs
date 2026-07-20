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
pub(super) use json::{json_table, markdown_table, run_jq};
#[cfg(test)]
pub(super) use media::{PgnPlyRequest, pgn_ply_request};

/// Trimmed paired-tag body, if the directive carries a non-empty one.
pub(super) fn inline_body(d: &Directive) -> Option<String> {
    d.inner
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A `![alt](key)` markdown image reference — the export-mode splice every
/// directive that resolves to an image (a synthesized SVG or a page-authored
/// file) uses instead of an HTML `<img>`, since mdcast's typst backend
/// converts real markdown images but not raw HTML.
pub(super) fn markdown_image(alt: &str, key: &str) -> String {
    format!("![{alt}]({key})")
}

pub(super) fn parse_size_class(d: &Directive) -> &'static str {
    match d.arg("size").unwrap_or("") {
        "small" | "sm" => " size-sm",
        "large" | "lg" => " size-lg",
        _ => "",
    }
}

/// Result of loading a stored file's bytes as text, distinguishing "the blob
/// row is gone" from "the blob exists but isn't valid UTF-8" — the two used to
/// collapse into the same `[... missing]` message, making a corrupt/binary
/// upload indistinguishable from a deleted one.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TextBlob {
    Found(String),
    NotFound,
    InvalidUtf8,
}

pub(super) async fn read_text_blob(db: &DatabaseConnection, hash: &str) -> TextBlob {
    decode_text_blob(files::read_blob(db, hash).await.ok().flatten())
}

pub(super) fn decode_text_blob(bytes: Option<Vec<u8>>) -> TextBlob {
    match bytes {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(s) => TextBlob::Found(s),
            Err(_) => TextBlob::InvalidUtf8,
        },
        None => TextBlob::NotFound,
    }
}
