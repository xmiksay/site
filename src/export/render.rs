//! The mdcast render entrypoint (#67, wiring together #64's `probe_pandoc`,
//! #65's `DbAssetProvider`, and #66's directive bridge into an actual
//! bytes-first render).
//!
//! mdcast ships its own embedded typst layouts / reveal.js dist
//! (`mdcast::EmbeddedAssets`), but this site's `design/` bundle has no
//! `design/mdcast/` mirror of that catalog yet — `DbAssetProvider` only
//! resolves template-prefixed keys against the design bundle and falls
//! through to `file_blobs` (which will never have them), so on its own it
//! can't answer a fallback layout request. `render_page` layers
//! `EmbeddedAssets` in as the base so every export works out of the box;
//! `DbAssetProvider` sits `over` it so a deployment that *does* populate
//! `design/mdcast/...` can still override individual templates, and the
//! #66 bridge (`export::asset_provider`) layers on top of that so
//! synthesized fen/pgn/mermaid SVGs always win.

use std::sync::Arc;

use mdcast::backends::Registry;
use mdcast::pages::auto::classify;
use mdcast::{
    BrandHandle, BrandSpec, DefaultSplitter, DocMeta, EmbeddedAssets, LayeredAssets, PageSplitter,
    RenderedArtifact, ResolvedDoc, Target,
};
use minijinja::Environment;
use sea_orm::DatabaseConnection;

use crate::design::DesignStore;
use crate::export::{DbAssetProvider, asset_provider};
use crate::markdown;

/// The two export shapes this site exposes over HTTP. `mdcast` supports
/// more targets (DOCX/ODT/PPTX), but only PDF and reveal.js-slides are
/// wired to routes for now — see `docs/architecture.md#export-mdcast`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Slides,
}

impl ExportFormat {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pdf" => Some(Self::Pdf),
            "slides" => Some(Self::Slides),
            _ => None,
        }
    }

    pub fn target(self) -> Target {
        match self {
            Self::Pdf => Target::Pdf,
            Self::Slides => Target::HtmlReveal,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Slides => "text/html; charset=utf-8",
        }
    }

    /// `Target::HtmlReveal` shells out to `pandoc`; `Target::Pdf` compiles
    /// in-process via typst and never needs the binary.
    pub fn requires_pandoc(self) -> bool {
        matches!(self, Self::Slides)
    }
}

/// Render `markdown_src` (a page/menu-item body) to the requested export
/// format: bridges directives to plain markdown + synthesized diagram assets
/// (#66), splits/classifies into mdcast pages, and dispatches through
/// mdcast's bytes-first `Registry` — no temp file ever touches disk.
pub async fn render_page(
    db: &DatabaseConnection,
    design: &Arc<DesignStore>,
    tmpl: &Environment<'static>,
    markdown_src: &str,
    title: Option<String>,
    logged_in: bool,
    format: ExportFormat,
) -> anyhow::Result<RenderedArtifact> {
    let bridged = markdown::render_for_export(markdown_src, db, tmpl, logged_in).await;

    let brand = BrandSpec::default();
    let raw = DefaultSplitter.split(&bridged.markdown);
    let pages = classify(raw, &brand.auto_layout);

    let doc = ResolvedDoc {
        pages,
        meta: DocMeta {
            title,
            ..Default::default()
        },
        brand: BrandHandle(Arc::new(brand)),
        assets: Vec::new(),
        fonts: Vec::new(),
        toc: None,
    };

    let db_assets = DbAssetProvider::new(db.clone(), design.clone());
    let with_fallback = LayeredAssets {
        over: db_assets,
        base: EmbeddedAssets,
    };
    let assets = asset_provider(&bridged, with_fallback);

    Registry::with_defaults()
        .render_to_bytes(format.target(), &doc, &assets)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognizes_supported_formats_and_rejects_others() {
        assert_eq!(ExportFormat::parse("pdf"), Some(ExportFormat::Pdf));
        assert_eq!(ExportFormat::parse("slides"), Some(ExportFormat::Slides));
        assert_eq!(ExportFormat::parse("docx"), None);
        assert_eq!(ExportFormat::parse(""), None);
    }

    #[test]
    fn target_and_content_type_match_the_expected_mdcast_target() {
        assert_eq!(ExportFormat::Pdf.target(), Target::Pdf);
        assert_eq!(ExportFormat::Pdf.content_type(), "application/pdf");
        assert_eq!(ExportFormat::Slides.target(), Target::HtmlReveal);
        assert_eq!(
            ExportFormat::Slides.content_type(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn only_slides_requires_pandoc() {
        assert!(!ExportFormat::Pdf.requires_pandoc());
        assert!(ExportFormat::Slides.requires_pandoc());
    }
}
