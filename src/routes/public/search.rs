use axum::Router;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use axum_extra::extract::CookieJar;
use minijinja::context;
use sea_orm::EntityTrait;

use crate::auth;
use crate::entity::tag;
use crate::repo::pages as pages_repo;
use crate::routes::build_menu;
use crate::state::AppState;

use super::pages::PageView;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(search))
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;

pub async fn search(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<SearchQuery>,
) -> Html<String> {
    let logged_in = auth::is_logged_in(&state, &jar).await.is_some();
    let nav = build_menu(&state.db, logged_in).await;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);

    let q = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tag_name = query
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let path_prefix = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let result = pages_repo::search(
        &state.db,
        path_prefix,
        tag_name,
        q,
        logged_in,
        limit,
        offset,
    )
    .await;

    let (pages, total) = match result {
        Ok(r) => (
            r.pages.iter().map(PageView::from).collect::<Vec<_>>(),
            r.total,
        ),
        Err(pages_repo::SearchError::UnknownTag) => (Vec::new(), 0),
        Err(pages_repo::SearchError::Db(e)) => {
            return Html(format!("<h1>Database error</h1><pre>{e}</pre>"));
        }
    };

    let prev_offset = if offset == 0 {
        None
    } else {
        Some(offset.saturating_sub(limit))
    };
    let next_offset = if offset + limit < total {
        Some(offset + limit)
    } else {
        None
    };

    // Resolve tag (if filtering by name) for display
    let tag_model = if let Some(name) = tag_name {
        use sea_orm::{ColumnTrait, QueryFilter};
        tag::Entity::find()
            .filter(tag::Column::Name.eq(name))
            .one(&state.db)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let env = state.tmpl.env();
    let tmpl = match env.get_template("page_search.html") {
        Ok(t) => t,
        Err(e) => return Html(format!("<h1>Template error</h1><pre>{e}</pre>")),
    };

    match tmpl.render(context! {
        q => q.unwrap_or(""),
        tag_name => tag_name.unwrap_or(""),
        tag => tag_model,
        path_prefix => path_prefix.unwrap_or(""),
        pages,
        total,
        limit,
        offset,
        prev_offset,
        next_offset,
        menu_list => nav.list,
        menu_tree => nav.tree,
        logged_in,
    }) {
        Ok(html) => Html(html),
        Err(e) => Html(format!("<h1>Render error</h1><pre>{e}</pre>")),
    }
}
