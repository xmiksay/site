pub mod assets;
pub mod export;
pub mod images;
pub mod pages;
pub mod preview;
pub mod search;
pub mod sitemap;
pub mod tags;

use axum::extract::{Request, State};
use axum::response::{Html, Response};
use minijinja::Environment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{menu, page, tag};
use crate::markdown;
use crate::path_util;
use crate::routes::build_menu;
use crate::state::AppState;
use crate::templates::context::{Layout, PageView, PathPageContext, TagView};

use preview::Look;

/// Log the real error and serve a generic page — template paths and DB/SQL
/// detail must not reach anonymous visitors.
pub(super) fn error_page(context: &str, err: impl std::fmt::Display) -> Html<String> {
    tracing::error!("{context}: {err}");
    Html("<h1>Something went wrong</h1>".to_string())
}

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
pub async fn catch_all(State(state): State<AppState>, look: Look, req: Request) -> Response {
    let path = path_util::normalize(req.uri().path());
    let logged_in = look.logged_in;
    let layout = Layout::new(build_menu(&state.db, logged_in).await, logged_in);
    let env = look.env(&state);
    let rendered = render_path(&state, &env, &path, layout).await;
    look.respond("render error", rendered)
}

async fn render_path(
    state: &AppState,
    env: &Environment<'static>,
    path: &str,
    layout: Layout,
) -> Result<String, minijinja::Error> {
    let content = match lookup_content(&state.db, path).await {
        Some(content) if layout.logged_in || !content.private() => content,
        _ => return render_404(env, &layout),
    };
    let tmpl = env.get_template("path_page.html")?;
    let logged_in = layout.logged_in;
    let body_html = markdown::render(
        content.markdown(),
        &state.db,
        &state.storage,
        env,
        logged_in,
    )
    .await;
    let ctx = match content {
        PathContent::Menu(m) => menu_context(layout, &m, body_html),
        PathContent::Page(pg) => page_context(&state.db, layout, &pg, body_html).await,
    };
    tmpl.render(&ctx)
}

/// `path_page.html` context for a menu item.
pub(crate) fn menu_context(layout: Layout, m: &menu::Model, body_html: String) -> PathPageContext {
    PathPageContext {
        layout,
        body_html,
        page: None,
        breadcrumbs: Vec::new(),
        tags: Vec::new(),
        menu_id: Some(m.id),
    }
}

/// `path_page.html` context for a page; a failed tag lookup renders no tags.
pub(crate) async fn page_context(
    db: &DatabaseConnection,
    layout: Layout,
    pg: &page::Model,
    body_html: String,
) -> PathPageContext {
    let tags = tag::Entity::find()
        .filter(tag::Column::Id.is_in(pg.tag_ids.clone()))
        .all(db)
        .await
        .unwrap_or_default();
    PathPageContext {
        layout,
        body_html,
        page: Some(PageView::from(pg)),
        breadcrumbs: pages::breadcrumbs(&pg.path),
        tags: tags.into_iter().map(TagView::from).collect(),
        menu_id: None,
    }
}

fn render_404(env: &Environment<'static>, layout: &Layout) -> Result<String, minijinja::Error> {
    // A partial DESIGN_DIR bundle may lack 404.html — fall back rather than
    // fail every not-found.
    let tmpl = match env.get_template("404.html") {
        Err(e) if e.kind() == minijinja::ErrorKind::TemplateNotFound => {
            return Ok("<h1>Page not found</h1>".to_string());
        }
        tmpl => tmpl?,
    };
    tmpl.render(layout)
}
