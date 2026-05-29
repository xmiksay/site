pub mod images;
pub mod pages;
pub mod search;
pub mod sitemap;
pub mod tags;

use axum::extract::{Request, State};
use axum::response::Html;
use axum_extra::extract::CookieJar;
use minijinja::context;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::entity::{menu, page, tag};
use crate::path_util;
use crate::routes::{build_menu, Menu};
use crate::state::AppState;
use crate::{auth, markdown};

/// Catch-all handler: menu -> page -> 404
pub async fn catch_all(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
) -> Html<String> {
    let path = path_util::normalize(req.uri().path());
    let logged_in = auth::is_logged_in(&state, &jar).await.is_some();
    let nav = build_menu(&state.db, logged_in).await;

    let tmpl = match state.tmpl.get_template("path_page.html") {
        Ok(t) => t,
        Err(e) => return Html(format!("<h1>Template error</h1><pre>{e}</pre>")),
    };

    // 1. Menu hit covers page
    if let Ok(Some(menu_item)) = menu::Entity::find()
        .filter(menu::Column::Path.eq(&path))
        .one(&state.db)
        .await
    {
        if menu_item.private && !logged_in {
            return render_404(&state, &nav, logged_in);
        }
        let body_html = markdown::render(&menu_item.markdown, &state.db, &state.tmpl, logged_in).await;
        return match tmpl.render(context! {
            body_html,
            menu_list => nav.list,
            menu_tree => nav.tree,
            logged_in,
            menu_id => menu_item.id,
        }) {
            Ok(html) => Html(html),
            Err(e) => Html(format!("<h1>Render error</h1><pre>{e}</pre>")),
        };
    }

    // 2. Page fallback
    if let Ok(Some(pg)) = page::Entity::find()
        .filter(page::Column::Path.eq(&path))
        .one(&state.db)
        .await
    {
        if pg.private && !logged_in {
            return render_404(&state, &nav, logged_in);
        }

        let body_html = markdown::render(&pg.markdown, &state.db, &state.tmpl, logged_in).await;
        let page_view = pages::PageView::from(&pg);

        let tags = tag::Entity::find()
            .filter(tag::Column::Id.is_in(pg.tag_ids.clone()))
            .all(&state.db)
            .await
            .unwrap_or_default();

        return match tmpl.render(context! {
            page => page_view,
            breadcrumbs => pages::breadcrumbs(&pg.path),
            body_html,
            tags,
            menu_list => nav.list,
            menu_tree => nav.tree,
            logged_in,
        }) {
            Ok(html) => Html(html),
            Err(e) => Html(format!("<h1>Render error</h1><pre>{e}</pre>")),
        };
    }

    // 3. 404
    render_404(&state, &nav, logged_in)
}

fn render_404(state: &AppState, nav: &Menu, logged_in: bool) -> Html<String> {
    let tmpl = state.tmpl.get_template("404.html").unwrap();
    match tmpl.render(context! {
        menu_list => &nav.list,
        menu_tree => &nav.tree,
        logged_in,
    }) {
        Ok(html) => Html(html),
        Err(_) => Html("<h1>Page not found</h1>".to_string()),
    }
}
