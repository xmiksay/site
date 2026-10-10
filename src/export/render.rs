//! The export render entrypoint: bridge the site's markdown directives to
//! plain markdown + synthesized SVGs (#66), assemble the asset bundle, and
//! post the render to the remote `mdcast-server` through `mdcast-client`.
//!
//! The server runs the whole mdcast pipeline — frontmatter extraction, page
//! splitting, auto-classification against `BrandSpec::auto_layout`, and the
//! typst/pandoc backends — so the site sends raw multi-page markdown plus the
//! brand it loaded from `design/mdcast/brand.toml` (#68) and never splits or
//! classifies locally. Template overrides in the bundle shadow the server's
//! embedded catalog per mdcast 0.4's manifest semantics.
//!
//! One behavior delta vs. the in-process pipeline: the server extracts a
//! leading YAML frontmatter block, and its `title` beats the request's
//! `meta.title` (the page title passed here).

use mdcast_client::mdcast_api::{BrandSpec, Target};
use mdcast_client::{Artifact, Client, request};
use minijinja::Environment;
use sea_orm::DatabaseConnection;

use crate::design::Resolve;
use crate::export::{ExportError, build_bundle};
use crate::markdown;
use crate::storage::Storage;

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
}

/// Render `markdown_src` (a page/menu-item body) to the requested export
/// format on the remote render server: bridge directives to plain markdown +
/// synthesized diagram assets (#66), declare every referenced asset by
/// digest, and let `mdcast-client` negotiate which bytes actually upload.
#[allow(clippy::too_many_arguments)]
pub async fn render_page(
    client: &Client,
    db: &DatabaseConnection,
    storage: &Storage,
    design: &dyn Resolve,
    tmpl: &Environment<'static>,
    markdown_src: &str,
    title: Option<String>,
    logged_in: bool,
    format: ExportFormat,
) -> Result<Artifact, ExportError> {
    let bridged = markdown::render_for_export(markdown_src, db, storage, tmpl, logged_in).await;

    let brand = load_brand(design);
    let bundle = build_bundle(db, storage, design, &brand, &bridged).await?;

    let mut req = request::markdown(bridged.markdown, format.target());
    req.meta.title = title;
    req.brand = Some(brand);

    client.render(req, &bundle).await.map_err(Into::into)
}

/// Load the site's `BrandSpec` (#68) from `design/mdcast/brand.toml` —
/// resolved through the design view, so a `DESIGN_DIR` override applies
/// to it exactly like it does to templates. Unlike the template overrides
/// (`typst/…`, `revealjs/…`), this isn't an asset: it travels as
/// `request.brand`, caller-owned config the server hands to its splitter and
/// backends. A missing, non-UTF-8, or malformed file logs a warning and
/// degrades to `BrandSpec::default()` rather than failing the export.
fn load_brand(design: &dyn Resolve) -> BrandSpec {
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
    use crate::design::DesignStore;

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
