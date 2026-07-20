//! Export capability checks (#64 — foundation for the mdcast integration,
//! #63). `mdcast` renders PDF/PDF-slides (`Target::Pdf`/`PdfPresentation`)
//! in-process via the `typst`/`typst-as-lib` crates — no external `typst`
//! binary is ever spawned, so there is nothing to probe for it. DOCX/ODT/
//! PPTX/reveal.js-slides (`Target::HtmlReveal`, the epic's slice-1 slide
//! format) shell out to a `pandoc` subprocess, which *is* an external
//! runtime dependency and can be absent. `probe_pandoc` is the cheap
//! startup check that turns a missing binary into a typed, loggable error
//! instead of a panic the first time an export route tries to spawn it. The
//! public `/{*path}?format=...` and admin `/api/export/pages/{id}` routes
//! (#67) render through it. `assets` (#65) provides the DB-backed
//! `mdcast::AssetProvider` those routes render through. `bridge` (#66)
//! layers `markdown::render_for_export`'s synthesized fen/pgn/mermaid SVGs
//! over that provider. `render` (#67) is the actual render entrypoint the
//! routes call.

mod assets;
mod bridge;
mod render;

use std::fmt;

use tokio::process::Command;

pub use crate::markdown::BridgedMarkdown;
pub use assets::DbAssetProvider;
pub use bridge::asset_provider;
pub use render::{ExportFormat, render_page};

/// The configured pandoc binary could not be spawned or reported a failing
/// exit status. Never constructed from a panic — every path that can fail
/// (missing binary, spawn error, non-zero exit) is funneled through here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PandocUnavailable {
    pub binary: String,
    pub reason: String,
}

impl fmt::Display for PandocUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pandoc binary `{}` unavailable: {}",
            self.binary, self.reason
        )
    }
}

impl std::error::Error for PandocUnavailable {}

/// Confirm `binary` (`pandoc` unless overridden by `MDCAST_PANDOC_PATH`) is
/// on PATH and runnable. Cheap enough to call once at startup; callers must
/// not treat a failure as fatal — the rest of the site works fine without
/// export, so this degrades to a warning + typed error, never a panic/unwrap.
pub async fn probe_pandoc(binary: &str) -> Result<(), PandocUnavailable> {
    match Command::new(binary).arg("--version").output().await {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(PandocUnavailable {
            binary: binary.to_string(),
            reason: format!("exited with status {}", output.status),
        }),
        Err(err) => Err(PandocUnavailable {
            binary: binary.to_string(),
            reason: err.to_string(),
        }),
    }
}

/// Header-safe filename component shared by the public and admin export
/// routes — anything outside ASCII alnum/-/_ becomes `-`, so a page path can
/// never smuggle a CR/LF or quote into the `Content-Disposition` header
/// value.
pub(crate) fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_keeps_safe_chars_and_replaces_the_rest() {
        assert_eq!(sanitize_filename("about/team"), "about-team");
        assert_eq!(sanitize_filename("a_b-c123"), "a_b-c123");
        assert_eq!(sanitize_filename("héllo\r\n\""), "h-llo---");
    }

    #[tokio::test]
    async fn probe_pandoc_reports_missing_binary_without_panicking() {
        let err = probe_pandoc("mdcast-nonexistent-binary-xyz")
            .await
            .expect_err("a nonexistent binary must never be reported available");
        assert!(err.to_string().contains("mdcast-nonexistent-binary-xyz"));
    }

    #[test]
    fn mdcast_is_linked_with_the_configured_features() {
        assert_eq!(mdcast::Target::Pdf.extension(), "pdf");
        assert_eq!(mdcast::Target::HtmlReveal.extension(), "html");
    }
}
