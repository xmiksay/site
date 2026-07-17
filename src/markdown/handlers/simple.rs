//! `<page>`, `<file>`, `<image>`, `<gallery>` directives.

use minijinja::context;
use sea_orm::EntityTrait;

use crate::entity::file as file_entity;
use crate::repo::files::title_from_path;

use super::super::RenderCtx;
use super::super::directives::Directive;
use super::super::lookup::{
    GalleryLookup, PageLookup, fetch_file, fetch_gallery, fetch_page, lookup_label,
    parse_file_lookup, parse_gallery_lookup, parse_page_lookup,
};
use super::super::renderer::{
    block, expand_directives, render_expanded_to_html, render_md_template,
};

// ---------------------------------------------------------------------------
// <page path|id=...>
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_page(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_page_lookup(d) {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(page) = fetch_page(ctx.db, &lookup).await else {
        let label = match &lookup {
            PageLookup::Id(i) => i.to_string(),
            PageLookup::Path(p) => p.clone(),
        };
        let html = format!(r#"<p><em>[page "{label}" not found]</em></p>"#);
        return block(html);
    };

    if page.private && !ctx.logged_in {
        return String::new();
    }

    let path = page.path.clone();
    if ctx.visited_pages.contains(&path) {
        let html = format!(r#"<p><em>[recursive transclusion of "{path}" skipped]</em></p>"#);
        return block(html);
    }

    ctx.visited_pages.insert(path.clone());
    let nested = expand_directives(&page.markdown, ctx).await;
    ctx.visited_pages.remove(&path);

    let inner_html = render_expanded_to_html(&nested);

    let html = render_md_template(
        ctx,
        "page",
        context! { path => &path, inner_html => &inner_html },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <file path|id|hash=...>  — image if mime image/*, else download link
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_file(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_file_lookup(d, "file") {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(file) = fetch_file(ctx.db, &lookup).await else {
        let label = lookup_label(&lookup);
        let html = format!(r#"<p><em>[file "{label}" not found]</em></p>"#);
        return block(html);
    };

    let title = title_from_path(&file.path);
    if file.mimetype.starts_with("image/") {
        let html = render_md_template(
            ctx,
            "img",
            context! { hash => &file.hash, title => &title, alt => &title },
        );
        return block(html);
    }

    let description = file
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(title.as_str());
    let html = render_md_template(
        ctx,
        "file",
        context! { hash => &file.hash, title => &title, description },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <image path|id|hash=... alt=...>  — force image embed
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_img(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_file_lookup(d, "image") {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(file) = fetch_file(ctx.db, &lookup).await else {
        let label = lookup_label(&lookup);
        let html = format!(r#"<p><em>[image "{label}" not found]</em></p>"#);
        return block(html);
    };

    let title = title_from_path(&file.path);
    let alt = d
        .arg("alt")
        .filter(|s| !s.is_empty())
        .unwrap_or(title.as_str());
    let html = render_md_template(
        ctx,
        "img",
        context! { hash => &file.hash, title => &title, alt },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <gallery path|id=...>
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_gallery(
    d: &Directive,
    ctx: &mut RenderCtx<'_>,
) -> String {
    let lookup = match parse_gallery_lookup(d) {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(gal) = fetch_gallery(ctx.db, &lookup).await else {
        let label = match &lookup {
            GalleryLookup::Id(i) => i.to_string(),
            GalleryLookup::Path(p) => p.clone(),
        };
        let html = format!(r#"<p><em>[gallery "{label}" not found]</em></p>"#);
        return block(html);
    };

    #[derive(serde::Serialize)]
    struct GalleryItem {
        hash: String,
        title: String,
    }

    let mut items: Vec<GalleryItem> = Vec::with_capacity(gal.file_ids.len());
    for file_id in &gal.file_ids {
        if let Ok(Some(img)) = file_entity::Entity::find_by_id(*file_id).one(ctx.db).await {
            items.push(GalleryItem {
                hash: img.hash,
                title: title_from_path(&img.path),
            });
        }
    }

    let html = render_md_template(
        ctx,
        "gallery",
        context! { id => gal.id, title => &gal.title, items => &items },
    );
    block(html)
}
