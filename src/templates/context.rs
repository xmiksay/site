//! Typed render contexts — one struct per page template and directive
//! partial. These are the single source of truth for what a template
//! receives: the doc comments become the descriptions in
//! `docs/design-contract.md` (see [`super::contract`]).
//!
//! Every field is always serialized (an absent value is `none`, never
//! undefined) so templates render under `UndefinedBehavior::Strict`.

use schemars::JsonSchema;
use serde::Serialize;

use crate::entity::{page, tag};
use crate::routes::{Menu, MenuItem, MenuNode};

/// Variables every page template gets (they all extend `base.html`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Layout {
    /// Visible menu entries in `order_index` order, flat.
    pub menu_list: Vec<MenuItem>,
    /// The same entries nested by path prefix (`/a` contains `/a/b`).
    pub menu_tree: Vec<MenuNode>,
    /// Whether the visitor has an admin session (private items are included).
    pub logged_in: bool,
}

impl Layout {
    pub fn new(nav: Menu, logged_in: bool) -> Self {
        Self {
            menu_list: nav.list,
            menu_tree: nav.tree,
            logged_in,
        }
    }
}

/// `path_page.html` — a menu item or a page resolved from the request path.
/// Exactly one of `page` (a page) or `menu_id` (a menu item) is set.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PathPageContext {
    #[serde(flatten)]
    pub layout: Layout,
    /// The page's markdown rendered to HTML, directives expanded. Output it with `| safe`.
    pub body_html: String,
    /// The page, or `none` when the path resolved to a menu item.
    pub page: Option<PageView>,
    /// Cumulative path segments of the page (empty for a menu item).
    pub breadcrumbs: Vec<Crumb>,
    /// Tags attached to the page (empty for a menu item).
    pub tags: Vec<TagView>,
    /// Id of the menu item, or `none` when the path resolved to a page.
    pub menu_id: Option<i32>,
}

/// `page_search.html` — `/search` with its query, filters and one result page.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PageSearchContext {
    #[serde(flatten)]
    pub layout: Layout,
    /// Full-text query (`?q=`), empty when absent.
    pub q: String,
    /// Tag filter as typed (`?tag=`), empty when absent.
    pub tag_name: String,
    /// The tag named by `tag_name`, or `none` when absent or unknown.
    pub tag: Option<TagView>,
    /// Path prefix filter (`?path=`), empty when absent.
    pub path_prefix: String,
    /// The current result page.
    pub pages: Vec<PageView>,
    /// Number of matches across all result pages.
    pub total: u64,
    /// Page size (1–100, default 20).
    pub limit: u64,
    /// Index of the first result shown.
    pub offset: u64,
    /// Offset of the previous result page, `none` on the first.
    pub prev_offset: Option<u64>,
    /// Offset of the next result page, `none` on the last.
    pub next_offset: Option<u64>,
}

/// A page as templates see it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PageView {
    /// Page id (admin editor at `/admin/pages/{id}/edit`).
    pub id: i32,
    /// Canonical path without a leading slash; the page is served at `/{path}`.
    pub path: String,
    /// Short description for listings.
    pub summary: Option<String>,
    /// Ids of the page's tags.
    pub tag_ids: Vec<i32>,
    /// Private pages are only rendered for logged-in visitors.
    pub private: bool,
    /// `YYYY-MM-DD HH:MM:SS[.f] ±HH:MM` (e.g. `2026-03-04 12:30:00.123456 +00:00`) — format it with the `timeformat` filter.
    pub created_at: String,
    /// `YYYY-MM-DD HH:MM:SS[.f] ±HH:MM` (e.g. `2026-03-04 12:30:00.123456 +00:00`) — format it with the `timeformat` filter.
    pub modified_at: String,
}

impl From<&page::Model> for PageView {
    fn from(p: &page::Model) -> Self {
        Self {
            id: p.id,
            path: p.path.clone(),
            summary: p.summary.clone(),
            tag_ids: p.tag_ids.clone(),
            private: p.private,
            created_at: p.created_at.to_string(),
            modified_at: p.modified_at.to_string(),
        }
    }
}

/// One breadcrumb: `a/b` yields `{a, /a}` then `{b, /a/b}`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Crumb {
    /// The path segment.
    pub label: String,
    /// Absolute URL of the path up to this segment.
    pub href: String,
}

/// A tag; its listing lives at `/tag/{id}`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TagView {
    /// Tag id; `/tag/{id}` redirects to its search listing.
    pub id: i32,
    /// Unique tag name.
    pub name: String,
    /// Optional longer description.
    pub description: Option<String>,
}

impl From<tag::Model> for TagView {
    fn from(t: tag::Model) -> Self {
        Self {
            id: t.id,
            name: t.name,
            description: t.description,
        }
    }
}

/// `markdown/page.html` — `<page>` transclusion of another page.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PagePartial {
    /// Path of the transcluded page.
    pub path: String,
    /// Its rendered HTML. Output it with `| safe`.
    pub inner_html: String,
}

/// `markdown/img.html` — `<image>`, or `<file>` of an `image/*` file.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImgPartial {
    /// SHA-256 of the file: full size at `/files/{hash}`, thumbnail at `/files/{hash}/nahled`.
    pub hash: String,
    /// Last segment of the file's path.
    pub title: String,
    /// The `alt` attribute, defaulting to `title`.
    pub alt: String,
}

/// `markdown/file.html` — `<file>` of a non-image file (a download link).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FilePartial {
    /// SHA-256 of the file, served at `/files/{hash}`.
    pub hash: String,
    /// Last segment of the file's path.
    pub title: String,
    /// The file's description, defaulting to `title`.
    pub description: String,
}

/// `markdown/gallery.html` — `<gallery>`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GalleryPartial {
    /// The gallery's database id (e.g. to group its lightbox).
    pub id: i32,
    /// Gallery title.
    pub title: String,
    /// The gallery's files in order; empty for an empty gallery.
    pub items: Vec<GalleryItem>,
}

/// One gallery file.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GalleryItem {
    /// SHA-256 of the file: full size at `/files/{hash}`, thumbnail at `/files/{hash}/nahled`.
    pub hash: String,
    /// Last segment of the file's path.
    pub title: String,
    /// The file's full path.
    pub path: String,
}

/// `markdown/fen.html` — `<fen>`, a static chess position.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FenPartial {
    /// The FEN string.
    pub fen: String,
    /// `""`, `" size-sm"` or `" size-lg"` — append it to a class attribute.
    pub size_class: String,
}

/// `markdown/pgn.html` — `<pgn>`, a playable game.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PgnPartial {
    /// The PGN text.
    pub pgn: String,
    /// `""`, `" size-sm"` or `" size-lg"` — append it to a class attribute.
    pub size_class: String,
    /// The `move` attribute as written (`N`, `first`, `last`), `none` when absent.
    #[serde(rename = "move")]
    pub move_attr: Option<String>,
}

/// `markdown/mermaid.html` — `<mermaid>` or a ```` ```mermaid ```` fence.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MermaidPartial {
    /// Server-rendered SVG; empty when rendering failed (then show `source`). Output it with `| safe`.
    pub svg: String,
    /// The Mermaid source.
    pub source: String,
    /// `""`, `" size-sm"` or `" size-lg"` — append it to a class attribute.
    pub size_class: String,
}

/// `markdown/json.html` — `<json>`, a jq query result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct JsonPartial {
    /// Render type; currently always `table`.
    pub kind: String,
    /// Column headers (empty when the result has no object rows).
    pub columns: Vec<String>,
    /// Cell text, one list per row.
    pub rows: Vec<Vec<String>>,
}
