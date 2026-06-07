use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// Files baked into the binary at compile time from the `assets/` folder.
#[derive(Embed)]
#[folder = "assets"]
struct Assets;

/// Runtime override of the baked-in assets, supplied via a folder on disk.
///
/// Lets a deployment ship its namespace (templates, css, js, img) as a plain
/// folder instead of having it compiled into the binary.
enum Overlay {
    /// No override folder configured — only the baked-in assets are used.
    None,
    /// Debug builds: read straight from disk on every request, so edits show
    /// up without a rebuild (transparent, like rust-embed in debug mode).
    Live(PathBuf),
    /// Release builds: the override folder is frozen into RAM at startup, so
    /// requests never touch the disk.
    Frozen(HashMap<String, Vec<u8>>),
}

/// Resolves resource requests against an optional override folder, then the
/// baked-in namespace, then the baked-in `common` folder.
pub struct AssetStore {
    namespace: String,
    overlay: Overlay,
}

impl AssetStore {
    /// Build a store for `namespace`, optionally overlaid by `override_dir`.
    ///
    /// In debug builds the override folder is read live on each request; in
    /// release builds it is frozen into RAM up front.
    pub fn new(namespace: String, override_dir: Option<PathBuf>) -> Self {
        let overlay = match override_dir {
            None => Overlay::None,
            Some(dir) if cfg!(debug_assertions) => {
                tracing::info!("assets: live override folder {}", dir.display());
                Overlay::Live(dir)
            }
            Some(dir) => {
                let frozen = freeze_dir(&dir);
                tracing::info!(
                    "assets: froze {} file(s) from {} into RAM",
                    frozen.len(),
                    dir.display()
                );
                Overlay::Frozen(frozen)
            }
        };
        Self { namespace, overlay }
    }

    /// Names of every template under `templates/`, deduplicated across the
    /// override folder, the baked namespace, and baked `common`. Used to
    /// compile all templates up front in release builds.
    pub fn template_names(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        let prefixes = [
            format!("{}/templates/", self.namespace),
            "common/templates/".to_string(),
        ];
        for file in Assets::iter() {
            for prefix in &prefixes {
                if let Some(rest) = file.strip_prefix(prefix.as_str()) {
                    names.insert(rest.to_string());
                    break;
                }
            }
        }
        self.overlay.template_names(&mut names);
        names.into_iter().collect()
    }

    /// Resolve a resource: override folder → baked namespace → baked common.
    pub fn load(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(data) = self.overlay.get(path) {
            return Some(data);
        }
        if let Some(file) = Assets::get(&format!("{}/{path}", self.namespace)) {
            return Some(file.data.into_owned());
        }
        Assets::get(&format!("common/{path}")).map(|file| file.data.into_owned())
    }
}

impl Overlay {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Overlay::None => None,
            Overlay::Live(dir) => std::fs::read(safe_join(dir, path)?).ok(),
            Overlay::Frozen(map) => map.get(path).cloned(),
        }
    }

    fn template_names(&self, names: &mut BTreeSet<String>) {
        match self {
            Overlay::None => {}
            Overlay::Frozen(map) => {
                for key in map.keys() {
                    if let Some(rest) = key.strip_prefix("templates/") {
                        names.insert(rest.to_string());
                    }
                }
            }
            Overlay::Live(dir) => {
                let base = dir.join("templates");
                collect_relative_files(&base, &base, names);
            }
        }
    }
}

/// Collect every file under `current` into `names` as a forward-slash path
/// relative to `base`.
fn collect_relative_files(base: &Path, current: &Path, names: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(base, &path, names);
        } else if let Ok(rel) = path.strip_prefix(base) {
            names.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Join `rel` onto `base`, rejecting absolute paths and `..` traversal so a
/// crafted request cannot escape the override folder.
fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(base.join(rel))
}

/// Recursively read every file under `dir` into a map keyed by forward-slash
/// relative path (matching the keys used by `AssetStore::load`).
fn freeze_dir(dir: &Path) -> HashMap<String, Vec<u8>> {
    let mut map = HashMap::new();
    freeze_into(dir, dir, &mut map);
    map
}

fn freeze_into(base: &Path, current: &Path, map: &mut HashMap<String, Vec<u8>>) {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("assets: cannot read {}: {e}", current.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            freeze_into(base, &path, map);
        } else if let (Ok(data), Ok(rel)) = (std::fs::read(&path), path.strip_prefix(base)) {
            map.insert(rel.to_string_lossy().replace('\\', "/"), data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_allows_normal_paths() {
        let base = Path::new("/srv/assets");
        assert_eq!(
            safe_join(base, "css/style.css"),
            Some(PathBuf::from("/srv/assets/css/style.css"))
        );
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute() {
        let base = Path::new("/srv/assets");
        assert_eq!(safe_join(base, "../secret"), None);
        assert_eq!(safe_join(base, "css/../../etc/passwd"), None);
        assert_eq!(safe_join(base, "/etc/passwd"), None);
    }

    #[test]
    fn load_falls_back_to_baked_common() {
        // No override folder: a known baked `common` template still resolves.
        let store = AssetStore::new("does-not-exist".into(), None);
        assert!(store.load("templates/base.html").is_some());
        assert!(store.load("templates/no-such-file.html").is_none());
    }

    #[test]
    fn overlay_takes_precedence_over_baked() {
        let dir = std::env::temp_dir().join("asset_store_overlay_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(dir.join("templates/base.html"), b"OVERRIDDEN").unwrap();

        let map = freeze_dir(&dir);
        let store = AssetStore {
            namespace: "common".into(),
            overlay: Overlay::Frozen(map),
        };
        assert_eq!(store.load("templates/base.html").as_deref(), Some(&b"OVERRIDDEN"[..]));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub fn build_asset_response(path: &str, data: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        data,
    )
        .into_response()
}
