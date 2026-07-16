use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// The default design bundle baked into the binary at compile time. This is
/// the always-present fallback layer; a deployment can override it at runtime
/// via the `DESIGN_DIR` folder.
#[derive(Embed)]
#[folder = "design"]
struct Baked;

/// Runtime override of the baked-in design, supplied via a folder on disk.
///
/// Lets a deployment ship its own design (templates, css, js, img) as a plain
/// folder instead of having it compiled into the binary.
enum Overlay {
    /// No override folder configured — only the baked-in design is used.
    None,
    /// Debug builds: read straight from disk on every request, so edits show
    /// up without a rebuild (transparent, like rust-embed in debug mode).
    Live(PathBuf),
    /// Release builds: the override folder is frozen into RAM at startup, so
    /// requests never touch the disk.
    Frozen(HashMap<String, Vec<u8>>),
}

/// Resolves resource requests against an optional override folder, falling
/// back to the baked-in default design bundle.
pub struct DesignStore {
    overlay: Overlay,
}

impl DesignStore {
    /// Build a store optionally overlaid by `override_dir`.
    ///
    /// In debug builds the override folder is read live on each request; in
    /// release builds it is frozen into RAM up front.
    pub fn new(override_dir: Option<PathBuf>) -> Self {
        let overlay = match override_dir {
            None => Overlay::None,
            Some(dir) if cfg!(debug_assertions) => {
                tracing::info!("design: live override folder {}", dir.display());
                Overlay::Live(dir)
            }
            Some(dir) => {
                let frozen = freeze_dir(&dir);
                tracing::info!(
                    "design: froze {} file(s) from {} into RAM",
                    frozen.len(),
                    dir.display()
                );
                Overlay::Frozen(frozen)
            }
        };
        Self { overlay }
    }

    /// Names of every template under `templates/`, deduplicated across the
    /// override folder and the baked default. Used to compile all templates up
    /// front in release builds.
    pub fn template_names(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        for file in Baked::iter() {
            if let Some(rest) = file.strip_prefix("templates/") {
                names.insert(rest.to_string());
            }
        }
        self.overlay.template_names(&mut names);
        names.into_iter().collect()
    }

    /// Resolve a resource: override folder → baked default.
    pub fn load(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(data) = self.overlay.get(path) {
            return Some(data);
        }
        Baked::get(path).map(|file| file.data.into_owned())
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
/// relative path (matching the keys used by `DesignStore::load`).
fn freeze_dir(dir: &Path) -> HashMap<String, Vec<u8>> {
    let mut map = HashMap::new();
    freeze_into(dir, dir, &mut map);
    map
}

fn freeze_into(base: &Path, current: &Path, map: &mut HashMap<String, Vec<u8>>) {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("design: cannot read {}: {e}", current.display());
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
#[allow(clippy::items_after_test_module)] // pre-existing file layout; not touched by this change
mod tests {
    use super::*;

    #[test]
    fn safe_join_allows_normal_paths() {
        let base = Path::new("/srv/design");
        assert_eq!(
            safe_join(base, "css/style.css"),
            Some(PathBuf::from("/srv/design/css/style.css"))
        );
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute() {
        let base = Path::new("/srv/design");
        assert_eq!(safe_join(base, "../secret"), None);
        assert_eq!(safe_join(base, "css/../../etc/passwd"), None);
        assert_eq!(safe_join(base, "/etc/passwd"), None);
    }

    #[test]
    fn load_falls_back_to_baked_default() {
        // No override folder: a known baked template still resolves.
        let store = DesignStore::new(None);
        assert!(store.load("templates/base.html").is_some());
        assert!(store.load("templates/no-such-file.html").is_none());
    }

    #[test]
    fn load_resolves_assets_subfolder() {
        // Runtime static resources live under `assets/`; `/assets/<path>` maps to
        // `assets/<path>` in the bundle (see `serve_static`).
        let store = DesignStore::new(None);
        assert!(store.load("assets/css/style.css").is_some());
    }

    #[test]
    fn overlay_takes_precedence_over_baked() {
        let dir = std::env::temp_dir().join("design_store_overlay_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(dir.join("templates/base.html"), b"OVERRIDDEN").unwrap();

        let store = DesignStore {
            overlay: Overlay::Frozen(freeze_dir(&dir)),
        };
        assert_eq!(store.load("templates/base.html").as_deref(), Some(&b"OVERRIDDEN"[..]));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub fn build_static_response(path: &str, data: Vec<u8>) -> Response {
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
