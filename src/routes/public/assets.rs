//! `GET /assets/{*path}`: a runtime static resource from the design bundle's
//! `assets/` folder (`DESIGN_DIR` → storage override → baked), or from the
//! draft in preview mode (see [`preview`](super::preview)).

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;

use super::preview::Look;
use crate::design::build_static_response;
use crate::state::AppState;

pub async fn serve(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(path): Path<String>,
) -> Response {
    let look = match Look::resolve(&state, &jar).await {
        Ok(look) => look,
        Err(resp) => return *resp,
    };
    let key = format!("assets/{path}");
    let Some(data) = look.design(&state).load(&key) else {
        return look.finish((StatusCode::NOT_FOUND, "Not Found").into_response());
    };
    let mut resp = build_static_response(&key, data);
    // Assets are cached for a day; keyed on the cookies, turning preview on
    // or off misses that cache instead of serving the other design's copy.
    resp.headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Cookie"));
    look.finish(resp)
}
