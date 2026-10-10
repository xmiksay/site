//! Example contexts — the contract's examples and the smoke render's stand-in
//! for any template the site's real data does not exercise (e.g. no page
//! embeds a gallery yet). Typed, so they track the context structs.

use super::context::*;
use crate::routes::{MenuItem, MenuNode};

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

pub(super) fn layout(logged_in: bool) -> Layout {
    let item = |path: &str, label: &str| MenuItem {
        path: path.to_string(),
        label: label.to_string(),
    };
    let leaf = |path: &str, label: &str| MenuNode {
        path: path.to_string(),
        label: label.to_string(),
        children: Vec::new(),
    };
    Layout {
        menu_list: vec![
            item("/notes", "Notes"),
            item("/notes/rust", "Rust"),
            item("/about", "About"),
        ],
        menu_tree: vec![
            MenuNode {
                children: vec![leaf("/notes/rust", "Rust")],
                ..leaf("/notes", "Notes")
            },
            leaf("/about", "About"),
        ],
        logged_in,
    }
}

fn page_view() -> PageView {
    PageView {
        id: 7,
        path: "notes/rust".to_string(),
        summary: Some("Notes on Rust".to_string()),
        tag_ids: vec![1],
        private: false,
        created_at: "2026-01-02 10:00:00 +00:00".to_string(),
        modified_at: "2026-03-04 12:30:00.123456 +00:00".to_string(),
    }
}

fn tag_view() -> TagView {
    TagView {
        id: 1,
        name: "rust".to_string(),
        description: Some("The Rust language".to_string()),
    }
}

pub(super) fn path_page_page(logged_in: bool) -> PathPageContext {
    PathPageContext {
        layout: layout(logged_in),
        body_html: "<p>Hello <strong>world</strong></p>".to_string(),
        page: Some(page_view()),
        breadcrumbs: vec![
            Crumb {
                label: "notes".to_string(),
                href: "/notes".to_string(),
            },
            Crumb {
                label: "rust".to_string(),
                href: "/notes/rust".to_string(),
            },
        ],
        tags: vec![tag_view()],
        menu_id: None,
    }
}

pub(super) fn path_page_menu(logged_in: bool) -> PathPageContext {
    PathPageContext {
        layout: layout(logged_in),
        body_html: "<p>Welcome</p>".to_string(),
        page: None,
        breadcrumbs: Vec::new(),
        tags: Vec::new(),
        menu_id: Some(1),
    }
}

pub(super) fn page_search(logged_in: bool) -> PageSearchContext {
    PageSearchContext {
        layout: layout(logged_in),
        q: "rust".to_string(),
        tag_name: "rust".to_string(),
        tag: Some(tag_view()),
        path_prefix: "notes".to_string(),
        pages: vec![page_view()],
        total: 3,
        limit: 1,
        offset: 1,
        prev_offset: Some(0),
        next_offset: Some(2),
    }
}

/// A plain full-text query: no tag (the `q` branches), nothing found.
pub(super) fn page_search_query(logged_in: bool) -> PageSearchContext {
    PageSearchContext {
        tag_name: String::new(),
        tag: None,
        path_prefix: String::new(),
        pages: Vec::new(),
        total: 0,
        limit: 20,
        offset: 0,
        prev_offset: None,
        next_offset: None,
        ..page_search(logged_in)
    }
}

pub(super) fn page_partial() -> PagePartial {
    PagePartial {
        path: "notes/rust".to_string(),
        inner_html: "<p>Transcluded</p>".to_string(),
    }
}

pub(super) fn img_partial() -> ImgPartial {
    ImgPartial {
        hash: HASH.to_string(),
        title: "diagram.png".to_string(),
        alt: "Architecture".to_string(),
    }
}

pub(super) fn file_partial() -> FilePartial {
    FilePartial {
        hash: HASH.to_string(),
        title: "spec.pdf".to_string(),
        description: "The specification".to_string(),
    }
}

pub(super) fn gallery_partial() -> GalleryPartial {
    GalleryPartial {
        id: 3,
        title: "Holiday".to_string(),
        items: vec![GalleryItem {
            hash: HASH.to_string(),
            title: "beach.jpg".to_string(),
            path: "holiday/beach.jpg".to_string(),
        }],
    }
}

pub(super) fn fen_partial() -> FenPartial {
    FenPartial {
        fen: "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1".to_string(),
        size_class: " size-lg".to_string(),
    }
}

pub(super) fn pgn_partial() -> PgnPartial {
    PgnPartial {
        pgn: "1. e4 e5 2. Nf3 Nc6".to_string(),
        size_class: String::new(),
        move_attr: Some("2".to_string()),
    }
}

pub(super) fn mermaid_partial() -> MermaidPartial {
    MermaidPartial {
        svg: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_string(),
        source: "graph TD\n  A --> B".to_string(),
        size_class: String::new(),
    }
}

pub(super) fn json_partial() -> JsonPartial {
    JsonPartial {
        kind: "table".to_string(),
        columns: vec!["name".to_string(), "count".to_string()],
        rows: vec![vec!["a".to_string(), "1".to_string()]],
    }
}
