pub mod export;
pub mod images;
pub mod pages;
pub mod search;
pub mod sitemap;
pub mod tags;

use axum::extract::{Request, State};
use axum::response::Html;
use axum_extra::extract::CookieJar;
use minijinja::context;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{menu, page, tag};
use crate::path_util;
use crate::routes::{Menu, build_menu};
use crate::state::AppState;
use crate::{auth, markdown};

/// A path resolved to either a menu item or a page — the two content kinds
/// `catch_all` (and the export route) look up by path, in that priority
/// order.
pub(crate) enum PathContent {
    Menu(menu::Model),
    Page(page::Model),
}

impl PathContent {
    pub(crate) fn markdown(&self) -> &str {
        match self {
            Self::Menu(m) => &m.markdown,
            Self::Page(p) => &p.markdown,
        }
    }

    pub(crate) fn private(&self) -> bool {
        match self {
            Self::Menu(m) => m.private,
            Self::Page(p) => p.private,
        }
    }

    /// `page` has no `title` column (only `summary`), so this falls back to
    /// the page's own path.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Menu(m) => m.title.clone(),
            Self::Page(p) => p.path.clone(),
        }
    }
}

/// Shared menu -> page lookup used by both `catch_all` and the export route,
/// so the two stay in lockstep on lookup order and privacy semantics.
pub(crate) async fn lookup_content(db: &DatabaseConnection, path: &str) -> Option<PathContent> {
    if let Ok(Some(m)) = menu::Entity::find()
        .filter(menu::Column::Path.eq(path))
        .one(db)
        .await
    {
        return Some(PathContent::Menu(m));
    }
    if let Ok(Some(p)) = page::Entity::find()
        .filter(page::Column::Path.eq(path))
        .one(db)
        .await
    {
        return Some(PathContent::Page(p));
    }
    None
}

/// Catch-all handler: menu -> page -> 404
pub async fn catch_all(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
) -> Html<String> {
    let path = path_util::normalize(req.uri().path());
    let logged_in = auth::is_logged_in(&state, &jar).await.is_some();
    let nav = build_menu(&state.db, logged_in).await;

    let env = state.tmpl.env();
    let tmpl = match env.get_template("path_page.html") {
        Ok(t) => t,
        Err(e) => return Html(format!("<h1>Template error</h1><pre>{e}</pre>")),
    };

    let Some(content) = lookup_content(&state.db, &path).await else {
        return render_404(&state, &nav, logged_in);
    };

    match content {
        PathContent::Menu(menu_item) => {
            if menu_item.private && !logged_in {
                return render_404(&state, &nav, logged_in);
            }
            let body_html = markdown::render(&menu_item.markdown, &state.db, &env, logged_in).await;
            match tmpl.render(context! {
                body_html,
                menu_list => nav.list,
                menu_tree => nav.tree,
                logged_in,
                menu_id => menu_item.id,
            }) {
                Ok(html) => Html(html),
                Err(e) => Html(format!("<h1>Render error</h1><pre>{e}</pre>")),
            }
        }
        PathContent::Page(pg) => {
            if pg.private && !logged_in {
                return render_404(&state, &nav, logged_in);
            }

            let body_html = markdown::render(&pg.markdown, &state.db, &env, logged_in).await;
            let page_view = pages::PageView::from(&pg);

            let tags = tag::Entity::find()
                .filter(tag::Column::Id.is_in(pg.tag_ids.clone()))
                .all(&state.db)
                .await
                .unwrap_or_default();

            match tmpl.render(context! {
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
            }
        }
    }
}

fn render_404(state: &AppState, nav: &Menu, logged_in: bool) -> Html<String> {
    let env = state.tmpl.env();
    let tmpl = env.get_template("404.html").unwrap();
    match tmpl.render(context! {
        menu_list => &nav.list,
        menu_tree => &nav.tree,
        logged_in,
    }) {
        Ok(html) => Html(html),
        Err(_) => Html("<h1>Page not found</h1>".to_string()),
    }
}
