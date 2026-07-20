//! Wires `markdown::render_for_export`'s synthesized SVGs (fen/pgn/mermaid
//! diagrams — no `file_blobs` row of their own) into an `AssetProvider`
//! mdcast can render straight through, layered over `DbAssetProvider` for
//! everything else (page-authored images, design templates) (#66).

use std::collections::HashMap;

use anyhow::Result;
use bytes::Bytes;
use mdcast::{AssetProvider, LayeredAssets, sync_provider};

use crate::markdown::BridgedMarkdown;

/// `AssetProvider` for `bridged.markdown`: its synthesized SVGs (keyed
/// `bridge/{fen,pgn,mermaid}/...`) resolve first; anything else falls
/// through to `base` (typically a `DbAssetProvider`, but generic here so a
/// unit test can stand in a plain `sync_provider` instead).
pub fn asset_provider<B: AssetProvider>(
    bridged: &BridgedMarkdown,
    base: B,
) -> LayeredAssets<impl AssetProvider, B> {
    let synthesized: HashMap<String, Bytes> = bridged.assets.iter().cloned().collect();
    LayeredAssets {
        over: sync_provider(move |key: &str| -> Result<Option<Bytes>> {
            Ok(synthesized.get(key).cloned())
        }),
        base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridged_with(assets: Vec<(String, Bytes)>) -> BridgedMarkdown {
        BridgedMarkdown {
            markdown: String::new(),
            assets,
        }
    }

    #[tokio::test]
    async fn synthesized_asset_overrides_base() {
        let bridged = bridged_with(vec![(
            "bridge/fen/abc.svg".to_string(),
            Bytes::from_static(b"OVER"),
        )]);
        let base = sync_provider(|key: &str| {
            if key == "bridge/fen/abc.svg" {
                Ok(Some(Bytes::from_static(b"BASE")))
            } else {
                Ok(None)
            }
        });

        let provider = asset_provider(&bridged, base);
        let got = provider
            .get("bridge/fen/abc.svg")
            .await
            .expect("get must not error");
        assert_eq!(got, Some(Bytes::from_static(b"OVER")));
    }

    #[tokio::test]
    async fn key_absent_from_bridge_falls_through_to_base() {
        let bridged = bridged_with(vec![]);
        let base = sync_provider(|key: &str| {
            if key == "images/logo.svg" {
                Ok(Some(Bytes::from_static(b"BASE-LOGO")))
            } else {
                Ok(None)
            }
        });

        let provider = asset_provider(&bridged, base);
        let got = provider
            .get("images/logo.svg")
            .await
            .expect("get must not error");
        assert_eq!(got, Some(Bytes::from_static(b"BASE-LOGO")));
    }
}
