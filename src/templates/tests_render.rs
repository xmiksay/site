//! The typed contexts render the baked templates byte-identically to the
//! ad-hoc `context!{}` maps they replaced, and strictly (no undefined value).

use std::sync::Arc;

use minijinja::{Environment, context};
use serde::Serialize;

use super::{Templates, samples, strict_environment};
use crate::design::DesignStore;

fn lenient() -> Arc<Environment<'static>> {
    Templates::new(Arc::new(DesignStore::new(None))).env()
}

fn strict() -> Environment<'static> {
    let design = DesignStore::new(None);
    strict_environment(move |p| design.load(p))
}

/// Render `typed` leniently and strictly; both must equal the legacy render.
fn assert_identical(name: &str, typed: &impl Serialize, legacy: minijinja::Value) {
    let env = lenient();
    let tmpl = env.get_template(name).unwrap();
    let old = tmpl.render(legacy).unwrap();
    assert_eq!(tmpl.render(typed).unwrap(), old, "{name}: lenient");
    let strict = strict();
    let new = strict.get_template(name).unwrap().render(typed).unwrap();
    assert_eq!(new, old, "{name}: strict");
}

#[test]
fn layout_pages_render_identically() {
    for logged_in in [false, true] {
        let l = samples::layout(logged_in);
        let legacy = context! {
            menu_list => &l.menu_list, menu_tree => &l.menu_tree, logged_in,
        };
        assert_identical("404.html", &l, legacy.clone());
        assert_identical("base.html", &l, legacy);
    }
}

#[test]
fn path_page_renders_identically_for_pages_and_menu_items() {
    for logged_in in [false, true] {
        let p = samples::path_page_page(logged_in);
        let legacy = context! {
            page => &p.page, breadcrumbs => &p.breadcrumbs, body_html => &p.body_html,
            tags => &p.tags, menu_list => &p.layout.menu_list,
            menu_tree => &p.layout.menu_tree, logged_in,
        };
        assert_identical("path_page.html", &p, legacy);

        // A menu item used to get no `page`/`tags`/`breadcrumbs` at all.
        let m = samples::path_page_menu(logged_in);
        let legacy = context! {
            body_html => &m.body_html, menu_list => &m.layout.menu_list,
            menu_tree => &m.layout.menu_tree, logged_in, menu_id => m.menu_id,
        };
        assert_identical("path_page.html", &m, legacy);
    }
}

#[test]
fn page_search_renders_identically() {
    let mut s = samples::page_search(false);
    for (tag, prev, next) in [(true, Some(0), Some(2)), (false, None, None)] {
        if !tag {
            s.tag = None;
        }
        s.prev_offset = prev;
        s.next_offset = next;
        let legacy = context! {
            q => &s.q, tag_name => &s.tag_name, tag => &s.tag, path_prefix => &s.path_prefix,
            pages => &s.pages, total => s.total, limit => s.limit, offset => s.offset,
            prev_offset => s.prev_offset, next_offset => s.next_offset,
            menu_list => &s.layout.menu_list, menu_tree => &s.layout.menu_tree,
            logged_in => s.layout.logged_in,
        };
        assert_identical("page_search.html", &s, legacy);
    }
}

#[test]
fn partials_render_identically() {
    let p = samples::page_partial();
    let legacy = context! { path => &p.path, inner_html => &p.inner_html };
    assert_identical("markdown/page.html", &p, legacy);

    let i = samples::img_partial();
    let legacy = context! { hash => &i.hash, title => &i.title, alt => &i.alt };
    assert_identical("markdown/img.html", &i, legacy);

    let f = samples::file_partial();
    let legacy = context! { hash => &f.hash, title => &f.title, description => &f.description };
    assert_identical("markdown/file.html", &f, legacy);

    let mut g = samples::gallery_partial();
    for _ in 0..2 {
        let legacy = context! { id => g.id, title => &g.title, items => &g.items };
        assert_identical("markdown/gallery.html", &g, legacy);
        g.items.clear();
    }

    let f = samples::fen_partial();
    let legacy = context! { fen => &f.fen, size_class => &f.size_class };
    assert_identical("markdown/fen.html", &f, legacy);

    let mut p = samples::pgn_partial();
    for _ in 0..2 {
        let legacy = context! { pgn => &p.pgn, size_class => &p.size_class, move => &p.move_attr };
        assert_identical("markdown/pgn.html", &p, legacy);
        p.move_attr = None;
    }

    let mut m = samples::mermaid_partial();
    for _ in 0..2 {
        let legacy = context! { svg => &m.svg, source => &m.source, size_class => &m.size_class };
        assert_identical("markdown/mermaid.html", &m, legacy);
        m.svg.clear();
    }

    let j = samples::json_partial();
    let legacy = context! { kind => &j.kind, columns => &j.columns, rows => &j.rows };
    assert_identical("markdown/json.html", &j, legacy);
}

#[test]
fn strict_environment_rejects_undefined_values() {
    let mut env = strict();
    env.add_template("t.html", "{{ nope }}").unwrap();
    let err = env.get_template("t.html").unwrap().render(()).unwrap_err();
    assert_eq!(err.kind(), minijinja::ErrorKind::UndefinedError);
}
