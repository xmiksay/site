//! Builds the `AssetBundle` a remote mdcast render needs: every asset key
//! the document references, declared up front — the server never calls back
//! into the site, it only answers `409` with the digests its cache lacks.
//!
//! Three layers:
//! 1. **Bridge SVGs** (eager) — `render_for_export`'s synthesized
//!    fen/pgn/mermaid diagrams; in-memory already, no `files` row of their own.
//! 2. **Design templates** (eager) — everything under `design/mdcast/` except
//!    `brand.toml` (which travels as `request.brand`, not as an asset), keyed
//!    with the `mdcast/` prefix stripped so the entries shadow mdcast-server's
//!    embedded catalog (`typst/layouts/pdf/hero.typ`, `revealjs/brand.css`).
//!    ~32K total; a `DESIGN_DIR` edit changes the digest and re-uploads.
//! 3. **Page images** (digest-only, lazy) — image destinations in the bridged
//!    markdown resolved through `files`; `files.hash` is already the sha256
//!    the manifest wants, so blob bytes are only read if the server's cache
//!    actually misses the digest.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use bytes::Bytes;
use mdcast_client::AssetBundle;
use mdcast_client::mdcast_api::{BrandSpec, Digest};
use pulldown_cmark::{Event, Parser, Tag};
use sea_orm::DatabaseConnection;

use crate::design::DesignStore;
use crate::markdown::BridgedMarkdown;
use crate::markdown::lookup::{FileLookup, fetch_file};
use crate::storage::Storage;

/// `DesignStore` prefix holding the mdcast template overrides.
const DESIGN_PREFIX: &str = "mdcast/";
/// Brand config — sent as `request.brand`, never as an asset.
const BRAND_TOML: &str = "mdcast/brand.toml";

/// Assemble the bundle for one export render. A markdown image whose path
/// matches no `files` row is skipped with a debug log — the server drops
/// undeclared refs with a warning, the same soft failure the in-process
/// pipeline had.
pub async fn build_bundle(
    db: &DatabaseConnection,
    storage: &Storage,
    design: &DesignStore,
    brand: &BrandSpec,
    bridged: &BridgedMarkdown,
) -> Result<AssetBundle> {
    let mut bundle = AssetBundle::new();

    for (key, bytes) in &bridged.assets {
        bundle.insert(key.clone(), bytes.clone());
    }

    for (key, bytes) in design_templates(design) {
        bundle.insert(key, bytes);
    }

    let mut keys = image_keys(&bridged.markdown);
    if let Some(logo) = &brand.logo {
        keys.insert(logo.key.clone());
    }
    for key in keys {
        insert_db_image(db, storage, &mut bundle, key).await?;
    }

    Ok(bundle)
}

/// The design bundle's mdcast template overrides as `(asset key, bytes)`
/// pairs — `design/mdcast/{path}` → key `{path}`, `brand.toml` excluded.
fn design_templates(design: &DesignStore) -> Vec<(String, Bytes)> {
    design
        .list_prefix(DESIGN_PREFIX)
        .into_iter()
        .filter(|path| path != BRAND_TOML)
        .filter_map(|path| {
            let key = path.strip_prefix(DESIGN_PREFIX)?.to_string();
            // A live-overlay file can vanish between list and load; skipping
            // it degrades to the server's embedded default for that key.
            let bytes = design.load(&path)?;
            Some((key, Bytes::from(bytes)))
        })
        .collect()
}

/// Image destinations in `markdown` that name site content: bridge keys are
/// already in the bundle, absolute/external URLs are not ours to declare.
fn image_keys(markdown: &str) -> BTreeSet<String> {
    Parser::new(markdown)
        .filter_map(|event| match event {
            Event::Start(Tag::Image { dest_url, .. }) if is_content_key(&dest_url) => {
                Some(dest_url.to_string())
            }
            _ => None,
        })
        .collect()
}

fn is_content_key(dest: &str) -> bool {
    !(dest.is_empty()
        || dest.starts_with("bridge/")
        || dest.starts_with("http://")
        || dest.starts_with("https://")
        || dest.starts_with("data:")
        || dest.starts_with("//"))
}

/// Declare `key` digest-only from its `files` row; bytes are fetched from
/// storage only if the server reports the digest missing.
async fn insert_db_image(
    db: &DatabaseConnection,
    storage: &Storage,
    bundle: &mut AssetBundle,
    key: String,
) -> Result<()> {
    let Some(file) = fetch_file(db, &FileLookup::Path(key.clone())).await else {
        tracing::debug!(%key, "export image has no files row; leaving it undeclared");
        return Ok(());
    };
    let digest = Digest::parse(file.hash.clone())
        .with_context(|| format!("files.hash for `{key}` is not a lowercase sha256 digest"))?;

    let storage = storage.clone();
    let hash = file.hash;
    let fetch_key = key.clone();
    bundle.insert_digest(key, digest, move || {
        let (storage, hash, key) = (storage.clone(), hash.clone(), fetch_key.clone());
        async move {
            storage
                .get_blob(&hash)
                .await
                .map_err(std::io::Error::other)?
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("blob {hash} missing for asset key `{key}`"),
                    )
                })
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_keys_collects_content_paths_and_dedupes() {
        let md = "![a](images/one.png)\n\n![b](images/two.jpg)\n\n![c](images/one.png)";
        let keys = image_keys(md);
        assert_eq!(
            keys.into_iter().collect::<Vec<_>>(),
            vec!["images/one.png".to_string(), "images/two.jpg".to_string()]
        );
    }

    #[test]
    fn image_keys_skips_bridge_external_and_data_destinations() {
        let md = concat!(
            "![f](bridge/fen/abc.svg)\n\n",
            "![h](https://example.com/x.png)\n\n",
            "![i](http://example.com/y.png)\n\n",
            "![d](data:image/png;base64,AAAA)\n\n",
            "![p](//cdn.example.com/z.png)\n\n",
            "[link not image](docs/file.pdf)\n\n",
            "![ok](images/kept.png)"
        );
        let keys = image_keys(md);
        assert_eq!(
            keys.into_iter().collect::<Vec<_>>(),
            vec!["images/kept.png"]
        );
    }

    #[test]
    fn design_templates_strips_prefix_and_excludes_brand_toml() {
        let dir = std::env::temp_dir().join("export_bundle_design_templates_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mdcast/typst/layouts/pdf")).unwrap();
        std::fs::write(dir.join("mdcast/brand.toml"), b"name = \"x\"").unwrap();
        std::fs::write(dir.join("mdcast/typst/layouts/pdf/hero.typ"), b"HERO").unwrap();

        let design = DesignStore::new(Some(dir.clone()));
        let templates = design_templates(&design);

        assert!(
            templates
                .iter()
                .any(|(key, bytes)| key == "typst/layouts/pdf/hero.typ" && bytes == "HERO")
        );
        assert!(
            templates.iter().all(|(key, _)| !key.contains("brand.toml")),
            "brand.toml travels as request.brand, never as an asset"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn baked_design_bundle_yields_the_site_template_overrides() {
        let design = DesignStore::new(None);
        let keys: Vec<String> = design_templates(&design)
            .into_iter()
            .map(|(key, _)| key)
            .collect();

        assert!(keys.contains(&"typst/layouts/pdf/hero.typ".to_string()));
        assert!(keys.contains(&"revealjs/brand.css".to_string()));
        assert!(!keys.contains(&"brand.toml".to_string()));
    }
}
