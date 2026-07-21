//! The mdcast render entrypoint (#67, wiring together #64's `probe_pandoc`,
//! #65's `DbAssetProvider`, and #66's directive bridge into an actual
//! bytes-first render).
//!
//! mdcast ships its own embedded typst layouts / reveal.js dist
//! (`mdcast::EmbeddedAssets`); `render_page` layers `EmbeddedAssets` in as the
//! base so every export works out of the box even where `design/mdcast/`
//! (baked or `DESIGN_DIR`-overridden) has nothing to say, and `DbAssetProvider`
//! sits `over` it so the site's own overrides win per key. `design/mdcast/`
//! (#68) mirrors that catalog for the classes/keys this site's brand actually
//! themes: `brand.toml` (the `BrandSpec` loaded below), the brand-aware
//! `typst/layouts/pdf/{content,hero,callout,section-divider,thanks}.typ`
//! (`image-full.typ` has no themeable text/color, so it falls through to
//! mdcast's embedded default untouched), and a `revealjs/brand.css` escape
//! hatch layered onto mdcast's own palette/font → reveal.js CSS projection.
//! The #66 bridge (`export::asset_provider`) layers on top of all of that so
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

    let brand = load_brand(design);
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

/// Load the site's `BrandSpec` (#68) from `design/mdcast/brand.toml` —
/// resolved through `DesignStore::load`, so a `DESIGN_DIR` override applies
/// to it exactly like it does to templates. Unlike mdcast's own catalog keys
/// (`typst/…`, `revealjs/…`), this isn't fetched through the `AssetProvider`:
/// `BrandSpec` is caller-owned config handed to `ResolvedDoc` up front, not
/// something a backend requests mid-render. A missing, non-UTF-8, or
/// malformed file logs a warning and degrades to `BrandSpec::default()`
/// rather than failing the export.
fn load_brand(design: &DesignStore) -> BrandSpec {
    let Some(bytes) = design.load("mdcast/brand.toml") else {
        return BrandSpec::default();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        tracing::warn!("mdcast/brand.toml is not valid UTF-8; using default BrandSpec");
        return BrandSpec::default();
    };
    match BrandSpec::from_toml(text) {
        Ok(spec) => spec,
        Err(err) => {
            tracing::warn!(%err, "invalid mdcast/brand.toml; using default BrandSpec");
            BrandSpec::default()
        }
    }
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

    #[test]
    fn load_brand_parses_a_valid_override() {
        let dir = std::env::temp_dir().join("export_render_load_brand_valid_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mdcast")).unwrap();
        std::fs::write(
            dir.join("mdcast/brand.toml"),
            br##"
                name = "Test Brand"

                [palette]
                accent = "#123456"

                [fonts]
                sans = "Test Sans"
            "##,
        )
        .unwrap();

        let design = DesignStore::new(Some(dir.clone()));
        let brand = load_brand(&design);

        assert_eq!(brand.name, "Test Brand");
        assert_eq!(
            brand.palette.get("accent").map(String::as_str),
            Some("#123456")
        );
        assert_eq!(
            brand.fonts.get("sans").map(String::as_str),
            Some("Test Sans")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_brand_falls_back_to_default_on_malformed_toml() {
        let dir = std::env::temp_dir().join("export_render_load_brand_malformed_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mdcast")).unwrap();
        std::fs::write(dir.join("mdcast/brand.toml"), b"not = [valid toml").unwrap();

        let design = DesignStore::new(Some(dir.clone()));
        let brand = load_brand(&design);

        assert!(brand.palette.is_empty());
        assert!(brand.fonts.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn baked_brand_toml_parses_and_matches_the_site_palette() {
        let design = DesignStore::new(None);
        let brand = load_brand(&design);

        assert_eq!(
            brand.palette.get("background").map(String::as_str),
            Some("#f5f5f5")
        );
        assert_eq!(
            brand.palette.get("accent").map(String::as_str),
            Some("#2563eb")
        );
        assert_eq!(
            brand.fonts.get("sans").map(String::as_str),
            Some("New Computer Modern")
        );
    }
}
