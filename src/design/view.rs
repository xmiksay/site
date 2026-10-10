//! What a request renders with: the live design ([`DesignStore`]) or, in
//! draft preview (#116), a [`DraftSite`] — the draft view (draft over baked,
//! `DESIGN_DIR` ignored: it shows what a publish would put live) with its
//! own template environment.
//!
//! The draft site is cached and rebuilt only when its inputs change. Every
//! preview request re-lists `design-draft/` (downloading only objects whose
//! version moved, as [`stored`](super::stored) does), so an edit from any
//! surface — API, MCP, assistant or a bucket edit — shows on the next
//! request.

use std::sync::Arc;

use minijinja::Environment;

use super::DesignStore;
use super::draft::{DRAFT_PREFIX, read_meta};
use super::stored::{Cache, DesignError, Files, Stored, files_of, load_prefix};
use crate::storage::{Storage, Version};

/// Resolves design bundle resources (`templates/…`, `assets/…`, `mdcast/…`).
pub trait Resolve: Send + Sync {
    fn load(&self, path: &str) -> Option<Vec<u8>>;
    /// Every resource path starting with `prefix`, sorted.
    fn list_prefix(&self, prefix: &str) -> Vec<String>;
}

impl Resolve for DesignStore {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        DesignStore::load(self, path)
    }

    fn list_prefix(&self, prefix: &str) -> Vec<String> {
        DesignStore::list_prefix(self, prefix)
    }
}

impl Resolve for Files {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        self.get(path).map(|b| b.to_vec())
    }

    fn list_prefix(&self, prefix: &str) -> Vec<String> {
        self.keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect()
    }
}

/// The draft view, ready to render.
pub struct DraftSite {
    files: Arc<Files>,
    env: Arc<Environment<'static>>,
}

impl DraftSite {
    fn new(files: Files) -> Self {
        let files = Arc::new(files);
        // Templates compile lazily on first use and stay cached in the
        // environment, so a broken one fails only the pages that use it —
        // with the error a preview shows in full.
        let env = Arc::new(crate::templates::environment(files.clone()));
        Self { files, env }
    }

    pub fn env(&self) -> Arc<Environment<'static>> {
        self.env.clone()
    }

    pub fn files(&self) -> &Files {
        &self.files
    }
}

impl Resolve for DraftSite {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        self.files.load(path)
    }

    fn list_prefix(&self, prefix: &str) -> Vec<String> {
        self.files.list_prefix(prefix)
    }
}

/// The inputs a [`DraftSite`] was built from.
struct Stamp {
    /// Swapped wholesale by a reload; an uninitialized draft is this view.
    /// Held, so the pointer cannot be reused by another `Stored`.
    published: Arc<Stored>,
    /// The draft's objects and versions; `None` while uninitialized.
    draft: Option<Vec<(String, Option<Version>)>>,
}

impl PartialEq for Stamp {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.published, &other.published) && self.draft == other.draft
    }
}

#[derive(Default)]
pub(super) struct SiteCache(Option<(Stamp, Arc<DraftSite>)>);

fn draft_stamp(cache: &Cache) -> Vec<(String, Option<Version>)> {
    let mut stamp: Vec<_> = cache
        .iter()
        .map(|(path, f)| (path.clone(), f.version.clone()))
        .collect();
    stamp.sort_by(|a, b| a.0.cmp(&b.0));
    stamp
}

impl DesignStore {
    /// The draft view as a renderable site, rebuilt only when the draft or
    /// the published design changed since the last call.
    pub async fn draft_site(&self, storage: &Storage) -> Result<Arc<DraftSite>, DesignError> {
        let mut cache = self.draft.lock().await;
        let initialized = read_meta(storage).await?.is_some();
        if initialized {
            *cache = load_prefix(storage, DRAFT_PREFIX, &cache).await?;
        }
        let stamp = Stamp {
            published: self.stored.read().clone(),
            draft: initialized.then(|| draft_stamp(&cache)),
        };
        let mut cached = self.draft_site.lock();
        if let Some((built, site)) = &cached.0
            && *built == stamp
        {
            return Ok(site.clone());
        }
        let files = match initialized {
            true => files_of(&cache),
            false => files_of(&stamp.published.files),
        };
        let site = Arc::new(DraftSite::new(self.with_baked(&files)));
        cached.0 = Some((stamp, site.clone()));
        Ok(site)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn files_resolve_by_path_and_prefix() {
        let files: Files = [("assets/a.css", "a"), ("templates/x.html", "x")]
            .into_iter()
            .map(|(p, b)| (p.to_string(), Bytes::from(b)))
            .collect();
        assert_eq!(files.load("assets/a.css").as_deref(), Some(&b"a"[..]));
        assert_eq!(files.load("assets/b.css"), None);
        assert_eq!(files.list_prefix("templates/"), ["templates/x.html"]);
    }

    #[test]
    fn draft_site_renders_its_own_templates() {
        let store = DesignStore::new(None);
        let mut files = Files::new();
        files.insert(
            "templates/404.html".into(),
            Bytes::from("DRAFT {{ 1 + 1 }}"),
        );
        let site = DraftSite::new(store.with_baked(&files));
        let out = site
            .env()
            .get_template("404.html")
            .and_then(|t| t.render(()));
        assert_eq!(out.ok().as_deref(), Some("DRAFT 2"));
        assert!(
            site.load("assets/css/style.css").is_some(),
            "baked fallback"
        );
    }
}
