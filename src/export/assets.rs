//! DB-backed `mdcast::AssetProvider` (#65, foundation laid by #64).
//!
//! `mdcast` never touches the filesystem itself — every template, brand-config
//! file, and referenced image is fetched through this trait. Two distinct
//! namespaces share the single `get`/`list` key space:
//!
//! - **mdcast's own catalog** — the typst layouts, reveal.js dist, pandoc
//!   reference docs, and Lua filters it ships under `embedded/` and requests
//!   by convention (`typst/…`, `revealjs/…`, `reference/…`, `filters/…`, see
//!   `TEMPLATE_PREFIXES`). We mirror that layout under `design/mdcast/` in
//!   the site's own design bundle and resolve it through `DesignStore::load`
//!   — never a direct `std::fs` read — so a `DESIGN_DIR` override on a
//!   deployment applies to exported documents exactly like it does to public
//!   pages.
//! - **page-authored content** — image references in markdown bodies and
//!   `BrandSpec::logo` keys are plain paths into this site's own content
//!   tree, resolved the same way `<image>`/`<file>` directives do: through
//!   the content-addressed `file_blobs` table via `markdown::lookup`.
//!
//! `TEMPLATE_PREFIXES` is a fixed, known-small set, but a page author is free
//! to upload content at a colliding path (e.g. `reference/handout.pdf`), so
//! `get` tries the design bundle first for those prefixes and falls through
//! to `file_blobs` on a miss rather than treating it as a hard "not found" —
//! mdcast's own template keys never have a matching content row, so the
//! fallback is a no-op in the common case and only matters for the collision.
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use mdcast::{AssetProvider, BoxFuture};
use sea_orm::DatabaseConnection;

use crate::design::DesignStore;
use crate::files;
use crate::markdown::lookup::{FileLookup, fetch_file};

/// Prefixes mdcast's embedded catalog uses for its own templates/brand
/// config (mirrors `embedded/{typst,revealjs,reference,filters}` in the
/// `mdcast` crate) — anything under one of these is tried against the
/// design bundle first.
const TEMPLATE_PREFIXES: &[&str] = &["typst/", "revealjs/", "reference/", "filters/"];

fn is_template_key(key: &str) -> bool {
    TEMPLATE_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

/// The `DesignStore` path a template-namespace `AssetProvider` key maps to.
fn design_path(key: &str) -> String {
    format!("mdcast/{key}")
}

/// Resolves mdcast's asset requests against this site's own storage:
/// templates/brand config from the `design/` bundle (`DesignStore`), images
/// and other page-referenced files from the content-addressed `file_blobs`
/// table.
pub struct DbAssetProvider {
    db: DatabaseConnection,
    design: Arc<DesignStore>,
}

impl DbAssetProvider {
    pub fn new(db: DatabaseConnection, design: Arc<DesignStore>) -> Self {
        Self { db, design }
    }

    async fn fetch_content(&self, key: &str) -> Result<Option<Bytes>> {
        let Some(file) = fetch_file(&self.db, &FileLookup::Path(key.to_owned())).await else {
            return Ok(None);
        };
        let data = files::read_blob(&self.db, &file.hash)
            .await
            .with_context(|| format!("reading file_blobs row for asset key `{key}`"))?;
        Ok(data.map(Bytes::from))
    }
}

impl AssetProvider for DbAssetProvider {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            if is_template_key(key)
                && let Some(data) = self.design.load(&design_path(key))
            {
                return Ok(Some(Bytes::from(data)));
            }
            self.fetch_content(key).await
        })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            // Only the design-bundle namespace supports listing (mdcast uses
            // it to discover sibling files under a template/reveal.js
            // directory); page content has no such directory-listing use.
            if !is_template_key(prefix) {
                return Ok(Vec::new());
            }
            Ok(self
                .design
                .list_prefix(&design_path(prefix))
                .into_iter()
                .filter_map(|path| path.strip_prefix("mdcast/").map(str::to_owned))
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_template_key_recognizes_every_mdcast_prefix() {
        for prefix in TEMPLATE_PREFIXES {
            assert!(
                is_template_key(&format!("{prefix}some/file.ext")),
                "prefix `{prefix}` should be recognized"
            );
        }
    }

    #[test]
    fn is_template_key_leaves_page_content_paths_alone() {
        assert!(!is_template_key("images/diagram.png"));
        assert!(!is_template_key("branding/logo.svg"));
    }

    #[test]
    fn design_path_prefixes_with_mdcast() {
        assert_eq!(
            design_path("typst/layouts/pdf/default.typ"),
            "mdcast/typst/layouts/pdf/default.typ"
        );
    }

    #[test]
    fn design_store_resolves_a_fixture_template_under_the_mdcast_prefix() {
        let dir = std::env::temp_dir().join("export_assets_design_fixture_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mdcast/typst/layouts/pdf")).unwrap();
        std::fs::write(
            dir.join("mdcast/typst/layouts/pdf/default.typ"),
            b"#let brand = context.brand",
        )
        .unwrap();

        let store = DesignStore::new(Some(dir.clone()));
        let key = design_path("typst/layouts/pdf/default.typ");
        assert_eq!(
            store.load(&key).as_deref(),
            Some(&b"#let brand = context.brand"[..])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
