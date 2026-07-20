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
        self.list_prefix("templates/")
            .into_iter()
            .filter_map(|path| path.strip_prefix("templates/").map(str::to_owned))
            .collect()
    }

    /// Every resource path starting with `prefix`, deduplicated across the
    /// override folder and the baked default — the general form
    /// `template_names` specializes to `"templates/"`. Used by the export
    /// `AssetProvider` (#65) so mdcast's typst/reveal.js backends can
    /// discover sibling files under a design-bundle subtree via
    /// `AssetProvider::list`.
    pub fn list_prefix(&self, prefix: &str) -> Vec<String> {
        let mut names = BTreeSet::new();
        for file in Baked::iter() {
            if file.starts_with(prefix) {
                names.insert(file.to_string());
            }
        }
        self.overlay.list_prefix(prefix, &mut names);
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

    fn list_prefix(&self, prefix: &str, names: &mut BTreeSet<String>) {
        match self {
            Overlay::None => {}
            Overlay::Frozen(map) => {
                for key in map.keys() {
                    if key.starts_with(prefix) {
                        names.insert(key.clone());
                    }
                }
            }
            Overlay::Live(dir) => {
                // Scope the walk to the narrowest directory that can contain a
                // match instead of the whole override tree: a `/`-terminated
                // prefix names that directory directly; otherwise fall back to
                // its parent (the last path segment may be a partial name).
                // `template_names()`'s `"templates/"` case resolves to exactly
                // the old hardcoded `dir.join("templates")` scope.
                let scope = match prefix.strip_suffix('/') {
                    Some(d) => dir.join(d),
                    None => match prefix.rsplit_once('/') {
                        Some((parent, _)) => dir.join(parent),
                        None => dir.clone(),
                    },
                };
                let mut found = BTreeSet::new();
                collect_relative_files(dir, &scope, &mut found);
                names.extend(found.into_iter().filter(|f| f.starts_with(prefix)));
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
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
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
        assert_eq!(
            store.load("templates/base.html").as_deref(),
            Some(&b"OVERRIDDEN"[..])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_prefix_scopes_the_live_overlay_walk_to_the_matching_subtree() {
        // A nested, non-"templates/" prefix (e.g. the export AssetProvider's
        // `mdcast/typst/...` namespace, #65) must still resolve correctly once
        // the walk is scoped to avoid reading the whole override tree.
        let dir = std::env::temp_dir().join("design_store_list_prefix_scoping_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mdcast/typst/layouts/pdf")).unwrap();
        std::fs::write(dir.join("mdcast/typst/layouts/pdf/default.typ"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("assets/img")).unwrap();
        std::fs::write(dir.join("assets/img/unrelated.png"), b"y").unwrap();

        let store = DesignStore::new(Some(dir.clone()));
        assert_eq!(
            store.list_prefix("mdcast/typst/layouts/pdf/"),
            vec!["mdcast/typst/layouts/pdf/default.typ".to_string()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
