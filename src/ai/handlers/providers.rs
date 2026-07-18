use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, EntityTrait, ModelTrait, QueryOrder, Set};

use crate::entity::llm_provider;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct ProviderView {
    pub id: i32,
    pub label: String,
    pub kind: String,
    pub base_url: Option<String>,
    pub has_api_key: bool,
    /// Max simultaneously in-flight requests to this provider's endpoint
    /// (ADR-0111); `None` uses the client's own default.
    pub concurrency: Option<i32>,
    /// Requests-per-minute budget for this provider's endpoint; `None` uses
    /// the client's own default.
    pub rpm: Option<i32>,
    pub created_at: String,
}

impl From<&llm_provider::Model> for ProviderView {
    fn from(p: &llm_provider::Model) -> Self {
        Self {
            id: p.id,
            label: p.label.clone(),
            kind: p.kind.clone(),
            base_url: p.base_url.clone(),
            has_api_key: p.api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
            concurrency: p.concurrency,
            rpm: p.rpm,
            created_at: p.created_at.to_string(),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct CreateProvider {
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub concurrency: Option<i32>,
    #[serde(default)]
    pub rpm: Option<i32>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct UpdateProvider {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Double-`Option`: absent (`None`) leaves the stored value untouched;
    /// present as JSON `null` (`Some(None)`) clears it back to "use the
    /// client default"; present as a number (`Some(Some(n))`) sets it. Unlike
    /// `base_url`/`api_key` (where an empty string is the "clear" sentinel),
    /// there's no such sentinel for an integer, so this is a genuine
    /// nullable-patch field — `#[serde(default)]` alone can't tell "omitted"
    /// from "explicit null" for a plain `Option<i32>`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub concurrency: Option<Option<i32>>,
    /// See `concurrency`'s doc — same double-`Option` clear semantics.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub rpm: Option<Option<i32>>,
}

/// Deserialize a present field (including explicit JSON `null`) as `Some`,
/// so it's distinguishable from an omitted field (which `#[serde(default)]`
/// leaves as the outer `None`) — the standard double-`Option` patch pattern.
fn deserialize_some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

fn validate_kind(kind: &str) -> Result<(), ApiError> {
    match kind {
        "anthropic" | "ollama" | "gemini" => Ok(()),
        other => Err(ApiError::BadRequest(format!(
            "unsupported provider kind: {other} (expected anthropic/ollama/gemini)"
        ))),
    }
}

/// A provided `concurrency`/`rpm` budget must be a positive integer — `0` or
/// negative would either wedge every turn against a zero-permit semaphore or
/// silently mean "unset" to the catalog (`positive_u32`/`positive_usize` in
/// `src/ai/catalog.rs`), so reject it loudly instead of saving a value the
/// catalog would then ignore.
fn validate_budget(field: &str, value: Option<i32>) -> Result<(), ApiError> {
    match value {
        Some(n) if n <= 0 => Err(ApiError::BadRequest(format!(
            "{field} must be a positive integer"
        ))),
        _ => Ok(()),
    }
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ProviderView>>> {
    let rows = llm_provider::Entity::find()
        .order_by_asc(llm_provider::Column::Label)
        .all(&state.db)
        .await?;
    Ok(Json(rows.iter().map(ProviderView::from).collect()))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<ProviderView>> {
    let row = llm_provider::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ProviderView::from(&row)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateProvider>,
) -> ApiResult<(StatusCode, Json<ProviderView>)> {
    if input.label.trim().is_empty() {
        return Err(ApiError::BadRequest("label is required".into()));
    }
    validate_kind(&input.kind)?;
    if matches!(input.kind.as_str(), "anthropic" | "gemini")
        && input
            .api_key
            .as_deref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        return Err(ApiError::BadRequest(format!(
            "{} provider requires api_key",
            input.kind
        )));
    }
    if input.kind == "ollama"
        && input
            .base_url
            .as_deref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        return Err(ApiError::BadRequest(
            "ollama provider requires base_url".into(),
        ));
    }
    validate_budget("concurrency", input.concurrency)?;
    validate_budget("rpm", input.rpm)?;

    let saved = llm_provider::ActiveModel {
        label: Set(input.label),
        kind: Set(input.kind),
        api_key: Set(input.api_key.filter(|s| !s.is_empty())),
        base_url: Set(input.base_url.filter(|s| !s.is_empty())),
        concurrency: Set(input.concurrency),
        rpm: Set(input.rpm),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .map_err(|e| ApiError::Conflict(format!("could not save: {e}")))?;

    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    Ok((StatusCode::CREATED, Json(ProviderView::from(&saved))))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateProvider>,
) -> ApiResult<Json<ProviderView>> {
    if let Some(Some(c)) = input.concurrency {
        validate_budget("concurrency", Some(c))?;
    }
    if let Some(Some(r)) = input.rpm {
        validate_budget("rpm", Some(r))?;
    }
    let row = llm_provider::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut active: llm_provider::ActiveModel = row.into();
    if let Some(l) = input.label {
        active.label = Set(l);
    }
    if let Some(k) = input.api_key {
        active.api_key = Set(if k.is_empty() { None } else { Some(k) });
    }
    if let Some(u) = input.base_url {
        active.base_url = Set(if u.is_empty() { None } else { Some(u) });
    }
    if let Some(c) = input.concurrency {
        active.concurrency = Set(c);
    }
    if let Some(r) = input.rpm {
        active.rpm = Set(r);
    }
    let updated = active.update(&state.db).await?;
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    Ok(Json(ProviderView::from(&updated)))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let row = llm_provider::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    row.delete(&state.db).await?;
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_budget_rejects_zero_and_negative() {
        assert!(validate_budget("concurrency", Some(0)).is_err());
        assert!(validate_budget("rpm", Some(-1)).is_err());
    }

    #[test]
    fn validate_budget_accepts_a_positive_value_or_absence() {
        assert!(validate_budget("concurrency", Some(1)).is_ok());
        assert!(validate_budget("rpm", None).is_ok());
    }

    /// The double-`Option` deserialization is the whole mechanism behind
    /// "clear vs. leave untouched" in `update()` — lock down that an omitted
    /// key, an explicit `null`, and a real value deserialize to three
    /// different states.
    #[test]
    fn update_provider_distinguishes_omitted_null_and_present_concurrency() {
        let omitted: UpdateProvider = serde_json::from_str("{}").expect("valid empty patch");
        assert_eq!(omitted.concurrency, None);

        let cleared: UpdateProvider =
            serde_json::from_str(r#"{"concurrency": null}"#).expect("valid null patch");
        assert_eq!(cleared.concurrency, Some(None));

        let set: UpdateProvider =
            serde_json::from_str(r#"{"concurrency": 5}"#).expect("valid value patch");
        assert_eq!(set.concurrency, Some(Some(5)));
    }
}
