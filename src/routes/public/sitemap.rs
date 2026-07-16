use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::entity::{menu, page};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/sitemap.xml", get(sitemap))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn host_from(state: &AppState) -> String {
    std::env::var("PUBLIC_URL")
        .ok()
        .or_else(|| std::env::var("SELF_URL").ok())
        .unwrap_or_else(|| {
            let _ = state;
            String::from("http://localhost:3000")
        })
        .trim_end_matches('/')
        .to_string()
}

pub async fn sitemap(State(state): State<AppState>) -> Response {
    let base = host_from(&state);

    let menu_rows: Vec<(String,)> = match menu::Entity::find()
        .select_only()
        .column(menu::Column::Path)
        .filter(menu::Column::Private.eq(false))
        .order_by_asc(menu::Column::Path)
        .into_tuple()
        .all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("sitemap error: {e}"),
            )
                .into_response();
        }
    };

    let page_rows: Vec<(String, chrono::DateTime<chrono::FixedOffset>)> = match page::Entity::find()
        .select_only()
        .column(page::Column::Path)
        .column(page::Column::ModifiedAt)
        .filter(page::Column::Private.eq(false))
        .order_by_asc(page::Column::Path)
        .into_tuple()
        .all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("sitemap error: {e}"),
            )
                .into_response();
        }
    };

    // dedupe: menu path covers a page path with the same key
    let menu_paths: std::collections::HashSet<String> =
        menu_rows.iter().map(|(p,)| p.clone()).collect();

    let mut xml = String::with_capacity(8192);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");

    // root
    xml.push_str("  <url><loc>");
    xml.push_str(&xml_escape(&format!("{base}/")));
    xml.push_str("</loc></url>\n");

    for (path,) in &menu_rows {
        if path.is_empty() {
            continue;
        }
        xml.push_str("  <url><loc>");
        xml.push_str(&xml_escape(&format!("{base}/{path}")));
        xml.push_str("</loc></url>\n");
    }

    for (path, modified_at) in &page_rows {
        if path.is_empty() || menu_paths.contains(path) {
            continue;
        }
        xml.push_str("  <url><loc>");
        xml.push_str(&xml_escape(&format!("{base}/{path}")));
        xml.push_str("</loc><lastmod>");
        xml.push_str(&modified_at.to_rfc3339());
        xml.push_str("</lastmod></url>\n");
    }

    xml.push_str("</urlset>\n");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}
