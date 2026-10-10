use axum::Router;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum_extra::extract::CookieJar;
use sea_orm::EntityTrait;

use crate::auth;
use crate::entity::tag;
use crate::repo::pages_search::{self as pages_search_repo, SearchError};
use crate::routes::build_menu;
use crate::state::AppState;
use crate::templates::context::{Layout, PageSearchContext, PageView, TagView};

use super::error_page;
use super::preview::Look;

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
) -> Response {
    let look = match Look::resolve(&state, &jar).await {
        Ok(look) => look,
        Err(resp) => return *resp,
    };
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

    let result = pages_search_repo::search(
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
        Err(SearchError::UnknownTag) => (Vec::new(), 0),
        Err(SearchError::Db(e)) => {
            return error_page("search db error", e).into_response();
        }
    };

    let (prev_offset, next_offset) = page_window(offset, limit, total);

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

    let ctx = PageSearchContext {
        layout: Layout::new(nav, logged_in),
        q: q.unwrap_or("").to_string(),
        tag_name: tag_name.unwrap_or("").to_string(),
        tag: tag_model.map(TagView::from),
        path_prefix: path_prefix.unwrap_or("").to_string(),
        pages,
        total,
        limit,
        offset,
        prev_offset,
        next_offset,
    };
    let env = look.env(&state);
    let rendered = env
        .get_template("page_search.html")
        .and_then(|tmpl| tmpl.render(&ctx));
    look.respond("search render error", rendered)
}

/// `(prev_offset, next_offset)` around the result page at `offset`.
pub(crate) fn page_window(offset: u64, limit: u64, total: u64) -> (Option<u64>, Option<u64>) {
    let prev = (offset != 0).then(|| offset.saturating_sub(limit));
    let next = (offset + limit < total).then_some(offset + limit);
    (prev, next)
}

#[cfg(test)]
mod tests {
    use super::page_window;

    #[test]
    fn page_window_has_neighbours_only_where_results_exist() {
        assert_eq!(page_window(0, 20, 5), (None, None));
        assert_eq!(page_window(0, 20, 45), (None, Some(20)));
        assert_eq!(page_window(20, 20, 45), (Some(0), Some(40)));
        assert_eq!(page_window(40, 20, 45), (Some(20), None));
        assert_eq!(page_window(5, 20, 45), (Some(0), Some(25)));
    }
}
