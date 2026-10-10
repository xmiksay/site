//! Draft preview mode (#116): with the `design_preview` cookie *and* a valid
//! session, public pages, `/assets/*` and exports render from the draft
//! ([`DraftSite`]). Without a session the cookie is ignored entirely, so
//! anonymous visitors never see draft content. Preview responses carry
//! `Cache-Control: no-store` and, for HTML, a fixed "NÁHLED DRAFTU" banner;
//! a template error shows its name, line and message instead of the generic
//! error page.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::CookieJar;
use minijinja::Environment;

use crate::auth;
use crate::design::stored::{DesignError, status_error};
use crate::design::{DraftSite, Resolve};
use crate::state::AppState;

pub const PREVIEW_COOKIE: &str = "design_preview";

/// Clears the cookie and redirects back (`routes::api::design`).
pub const EXIT_PATH: &str = "/api/design/preview/exit";

/// The cookie alone, before any session check.
fn requested(jar: &CookieJar) -> bool {
    jar.get(PREVIEW_COOKIE).is_some_and(|c| c.value() == "1")
}

/// How a page renders: the visitor's session state and, in preview, the
/// draft. Extracting it refreshes the draft from storage, so an edit shows
/// on the next page load. Rejects with the ready 503 page when the draft
/// cannot be loaded.
pub struct Look {
    pub logged_in: bool,
    draft: Option<Arc<DraftSite>>,
}

/// [`Look`] for `/assets/*`: the session is only checked when the preview
/// cookie is set (`logged_in` is false otherwise), and the draft is the one
/// the last preview page load built — assets never queue on storage.
pub struct AssetLook(pub Look);

impl FromRequestParts<AppState> for Look {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let jar = CookieJar::from_headers(&parts.headers);
        let logged_in = auth::is_logged_in(state, &jar).await.is_some();
        Look::with(state, logged_in && requested(&jar), logged_in, true)
            .await
            .map_err(unavailable)
    }
}

impl FromRequestParts<AppState> for AssetLook {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let jar = CookieJar::from_headers(&parts.headers);
        let logged_in = requested(&jar) && auth::is_logged_in(state, &jar).await.is_some();
        Look::with(state, logged_in, logged_in, false)
            .await
            .map(AssetLook)
            .map_err(unavailable)
    }
}

impl Look {
    async fn with(
        state: &AppState,
        preview: bool,
        logged_in: bool,
        refresh: bool,
    ) -> Result<Self, DesignError> {
        let draft = match preview {
            false => None,
            true => Some(state.design.draft_site(&state.storage, refresh).await?),
        };
        Ok(Self { logged_in, draft })
    }

    pub fn env(&self, state: &AppState) -> Arc<Environment<'static>> {
        match &self.draft {
            None => state.tmpl.env(),
            Some(site) => site.env(),
        }
    }

    pub fn design<'a>(&'a self, state: &'a AppState) -> &'a dyn Resolve {
        match &self.draft {
            None => &*state.design,
            Some(site) => &**site,
        }
    }

    /// Mark a non-HTML response (asset, export, a plain error) as preview
    /// output.
    pub fn finish(&self, mut resp: Response) -> Response {
        if self.draft.is_some() {
            no_store(&mut resp);
        }
        resp
    }

    /// The response for a rendered page; `context` labels the published
    /// error log.
    pub fn respond(&self, context: &str, rendered: Result<String, minijinja::Error>) -> Response {
        match (self.draft.is_some(), rendered) {
            (false, Ok(html)) => Html(html).into_response(),
            (false, Err(e)) => super::error_page(context, e).into_response(),
            (true, Ok(html)) => preview_html(StatusCode::OK, &html),
            (true, Err(e)) => {
                tracing::debug!("draft preview: {e:#}");
                preview_html(StatusCode::INTERNAL_SERVER_ERROR, &template_error(&e))
            }
        }
    }
}

fn unavailable(e: DesignError) -> Response {
    tracing::error!("draft preview: loading the draft failed: {e}");
    let body = format!(
        "<h1>Draft preview unavailable</h1><p>{}</p>",
        escape(&status_error(&e))
    );
    preview_html(StatusCode::SERVICE_UNAVAILABLE, &body)
}

fn no_store(resp: &mut Response) {
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn preview_html(status: StatusCode, html: &str) -> Response {
    let mut resp = (status, Html(inject_banner(html))).into_response();
    no_store(&mut resp);
    resp
}

const BANNER: &str = concat!(
    r#"<div id="design-preview-banner" style="position:fixed;left:0;right:0;bottom:0;"#,
    r#"z-index:2147483647;padding:6px 12px;background:#b91c1c;color:#fff;"#,
    r#"font:bold 14px/1.4 system-ui,sans-serif;text-align:center">NÁHLED DRAFTU "#,
    r#"&middot; <a href="/api/design/preview/exit" style="color:#fff;text-decoration:underline">"#,
    r#"Ukončit náhled</a></div>"#,
);

/// Put the banner right after the `<body…>` tag, or in front of the
/// document when there is none: it must not depend on the draft's
/// templates, which are exactly what is being edited.
fn inject_banner(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let body_end = lower.match_indices("<body").find_map(|(at, _)| {
        let rest = &lower[at + "<body".len()..];
        let attrs_start = rest.chars().next()?;
        if attrs_start != '>' && attrs_start != '/' && !attrs_start.is_ascii_whitespace() {
            return None;
        }
        rest.find('>').map(|gt| at + "<body".len() + gt + 1)
    });
    let at = body_end.unwrap_or(0);
    let mut out = String::with_capacity(html.len() + BANNER.len());
    out.push_str(&html[..at]);
    out.push_str(BANNER);
    out.push_str(&html[at..]);
    out
}

/// Every template error in `err`'s chain (an error in an included or
/// extended template is wrapped by its parent's) with name, line, message
/// and the source excerpt.
fn template_error(err: &minijinja::Error) -> String {
    let mut out = String::from("<!DOCTYPE html><html><head><meta charset=\"utf-8\">");
    out.push_str("<title>Template error</title></head><body><h1>Template error</h1>");
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        match e.downcast_ref::<minijinja::Error>() {
            Some(e) => {
                let name = e.name().unwrap_or("(unknown template)");
                let line = e.line().map_or_else(|| "?".into(), |l| l.to_string());
                let msg = e
                    .detail()
                    .map_or_else(|| e.kind().to_string(), str::to_owned);
                out.push_str(&format!(
                    "<h2>{}, line {line}</h2><p>{}: {}</p><pre>{}</pre>",
                    escape(name),
                    escape(&e.kind().to_string()),
                    escape(&msg),
                    escape(&e.display_debug_info().to_string()),
                ));
            }
            None => out.push_str(&format!("<p>{}</p>", escape(&e.to_string()))),
        }
        cur = e.source();
    }
    out.push_str("</body></html>");
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_goes_right_after_the_body_tag() {
        let html = r#"<html><head><title>b</title></head><BODY class="x">hi</BODY></html>"#;
        let out = inject_banner(html);
        let at = out.find(BANNER).expect("banner");
        assert_eq!(
            &out[..at],
            r#"<html><head><title>b</title></head><BODY class="x">"#
        );
        assert!(out.ends_with("hi</BODY></html>"));
        assert!(BANNER.contains("NÁHLED DRAFTU") && BANNER.contains(EXIT_PATH));
    }

    #[test]
    fn banner_skips_lookalike_tags_and_falls_back_to_prepending() {
        let out = inject_banner("<bodyguard>x</bodyguard><body>y");
        assert_eq!(out, format!("<bodyguard>x</bodyguard><body>{BANNER}y"));
        assert_eq!(
            inject_banner("<p>fragment</p>"),
            format!("{BANNER}<p>fragment</p>")
        );
        assert_eq!(inject_banner("<body"), format!("{BANNER}<body"));
        assert_eq!(inject_banner(""), BANNER);
    }

    #[test]
    fn template_errors_show_name_line_and_message_escaped() {
        let mut env = Environment::new();
        let err = env
            .add_template("broken.html", "ok\n{% if x %}<b>never closed")
            .expect_err("syntax error");
        let page = template_error(&err);
        assert!(page.contains("broken.html, line"), "{page}");
        assert!(page.contains("syntax error"), "{page}");
        assert!(!page.contains("<b>never"), "source must be escaped: {page}");
    }
}
