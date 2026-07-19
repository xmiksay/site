//! Generation-override validation + `GenerationParams` construction for
//! `create`/`update` (#42). Split out of `mutate.rs` to keep that file under
//! the workspace's 400-line cap.

use entanglement_core::{GenerationParams, ReasoningEffort};

use crate::entity::{llm_model, llm_provider};
use crate::routes::api::error::{ApiError, ApiResult};

/// `raw` (`"low" | "medium" | "high"`) → `entanglement_provider::ReasoningEffort`
/// — the API boundary's own validation, so an unrecognized string surfaces a
/// `400` here rather than reaching the engine as an opaque `InMsg::SetGeneration`
/// the wire-level `serde` deserialization would already have rejected anyway
/// (kept explicit so the error message names the bad value).
pub(super) fn parse_reasoning_effort(raw: &str) -> ApiResult<ReasoningEffort> {
    match raw {
        "low" => Ok(ReasoningEffort::Low),
        "medium" => Ok(ReasoningEffort::Medium),
        "high" => Ok(ReasoningEffort::High),
        other => Err(ApiError::BadRequest(format!(
            "unknown reasoning_effort `{other}` (expected low|medium|high)"
        ))),
    }
}

/// A provided `max_output_tokens`/`thinking_budget_tokens` must be a positive
/// integer — `0` is meaningless (mirrors `providers.rs`'s `validate_budget`
/// for `concurrency`/`rpm`) and should be rejected loudly rather than passed
/// through to the engine unchanged.
pub(super) fn validate_generation_budget(field: &str, value: Option<u32>) -> ApiResult<()> {
    match value {
        Some(0) => Err(ApiError::BadRequest(format!(
            "{field} must be a positive integer"
        ))),
        _ => Ok(()),
    }
}

/// Build the partial `GenerationParams` for `InMsg::SetGeneration` from the
/// four knobs this site's UI exposes — `None` when none were given (nothing
/// to send; a `create`/`update` with no generation input is not a generation
/// change).
///
/// `model` gates `temperature`/`reasoning_effort`/`thinking_budget_tokens`
/// against the target model's `llm_models.supports_*` flags (#53) so an
/// unsupported knob is rejected here rather than reaching the provider,
/// which otherwise fails the whole turn with a 400. `model: None` means no
/// model could be resolved to gate against (e.g. a legacy session row with a
/// null `model_id`) — skip gating entirely, preserving pre-#53 behavior for
/// that edge case. `max_output_tokens` has no capability gate: every model
/// accepts it.
pub(super) fn generation_overrides(
    model: Option<&llm_model::Model>,
    temperature: Option<f32>,
    reasoning_effort: Option<&str>,
    max_output_tokens: Option<u32>,
    thinking_budget_tokens: Option<u32>,
) -> ApiResult<Option<GenerationParams>> {
    if temperature.is_none()
        && reasoning_effort.is_none()
        && max_output_tokens.is_none()
        && thinking_budget_tokens.is_none()
    {
        return Ok(None);
    }
    if let Some(model) = model {
        if temperature.is_some() && !model.supports_temperature {
            return Err(ApiError::BadRequest(format!(
                "model `{}` does not support temperature",
                model.label
            )));
        }
        if reasoning_effort.is_some() && !model.supports_reasoning_effort {
            return Err(ApiError::BadRequest(format!(
                "model `{}` does not support reasoning_effort",
                model.label
            )));
        }
        if thinking_budget_tokens.is_some() && !model.supports_thinking {
            return Err(ApiError::BadRequest(format!(
                "model `{}` does not support thinking_budget_tokens",
                model.label
            )));
        }
    }
    validate_generation_budget("max_output_tokens", max_output_tokens)?;
    validate_generation_budget("thinking_budget_tokens", thinking_budget_tokens)?;
    let reasoning_effort = reasoning_effort.map(parse_reasoning_effort).transpose()?;
    Ok(Some(GenerationParams {
        temperature,
        max_output_tokens,
        thinking_budget_tokens,
        reasoning_effort,
    }))
}

/// Re-derive the overrides that should survive a model switch (#54):
/// `rebind()` (`entanglement-core`) rebuilds the live session's `generation`
/// from the `ModelResolver`'s `ResolvedModel::generation`, which this site
/// always resolves to `None` (the resolver has no session handle to read the
/// prior value from) — so a bare `SetModel` wipes every knob unless something
/// resends them. `explicit` is this call's already-validated request (`None`
/// for a knob it didn't touch); `existing_*` is the row's value before this
/// call. A knob `explicit` sets wins outright (already checked against
/// `model` by [`generation_overrides`]); a carried-over knob is silently
/// dropped instead of rejected if `model` doesn't support it — it was never
/// actually requested on this call, just inherited from before the switch.
/// Returns `None` when nothing ends up set, matching `generation_overrides`'s
/// "nothing to send" convention.
pub(super) fn carry_forward_generation(
    model: &llm_model::Model,
    explicit: Option<GenerationParams>,
    existing_temperature: Option<f32>,
    existing_reasoning_effort: Option<&str>,
    existing_max_output_tokens: Option<u32>,
    existing_thinking_budget_tokens: Option<u32>,
) -> Option<GenerationParams> {
    let explicit = explicit.unwrap_or_default();
    let temperature = explicit
        .temperature
        .or_else(|| existing_temperature.filter(|_| model.supports_temperature));
    let reasoning_effort = explicit.reasoning_effort.or_else(|| {
        existing_reasoning_effort
            .filter(|_| model.supports_reasoning_effort)
            .and_then(|raw| parse_reasoning_effort(raw).ok())
    });
    let max_output_tokens = explicit.max_output_tokens.or(existing_max_output_tokens);
    let thinking_budget_tokens = explicit
        .thinking_budget_tokens
        .or_else(|| existing_thinking_budget_tokens.filter(|_| model.supports_thinking));

    if temperature.is_none()
        && reasoning_effort.is_none()
        && max_output_tokens.is_none()
        && thinking_budget_tokens.is_none()
    {
        return None;
    }
    Some(GenerationParams {
        temperature,
        max_output_tokens,
        thinking_budget_tokens,
        reasoning_effort,
    })
}

/// `update()`'s call site for [`carry_forward_generation`]: only relevant
/// when this call actually switches the model — a model-only `SetModel` is
/// exactly the case that wipes the live session's generation (#54). A call
/// that leaves `model_id` untouched never sends `SetModel`, so `rebind()`
/// never runs and `explicit` (this call's own request, already validated by
/// [`generation_overrides`]) is the whole answer, same as before #54.
pub(super) fn generation_after_model_switch(
    model_changed: &Option<(llm_model::Model, llm_provider::Model)>,
    generation_target: &Option<llm_model::Model>,
    explicit: Option<GenerationParams>,
    existing_temperature: Option<f32>,
    existing_reasoning_effort: Option<&str>,
    existing_max_output_tokens: Option<u32>,
    existing_thinking_budget_tokens: Option<u32>,
) -> Option<GenerationParams> {
    match (model_changed, generation_target) {
        (Some(_), Some(model)) => carry_forward_generation(
            model,
            explicit,
            existing_temperature,
            existing_reasoning_effort,
            existing_max_output_tokens,
            existing_thinking_budget_tokens,
        ),
        _ => explicit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture row for capability-gating tests — id/provider_id/label/model/
    /// is_default/context_window/created_at are arbitrary fixed values; only
    /// the three `supports_*` flags vary per test.
    fn fixture_model(
        supports_temperature: bool,
        supports_reasoning_effort: bool,
        supports_thinking: bool,
    ) -> llm_model::Model {
        llm_model::Model {
            id: 1,
            provider_id: 1,
            label: "fixture-model".to_string(),
            model: "fixture-model".to_string(),
            is_default: true,
            context_window: None,
            supports_temperature,
            supports_reasoning_effort,
            supports_thinking,
            created_at: chrono::Utc::now().fixed_offset(),
        }
    }

    #[test]
    fn validate_generation_budget_rejects_zero() {
        assert!(validate_generation_budget("max_output_tokens", Some(0)).is_err());
        assert!(validate_generation_budget("thinking_budget_tokens", Some(0)).is_err());
    }

    #[test]
    fn validate_generation_budget_accepts_positive_or_absent() {
        assert!(validate_generation_budget("max_output_tokens", Some(1)).is_ok());
        assert!(validate_generation_budget("thinking_budget_tokens", None).is_ok());
    }

    #[test]
    fn generation_overrides_none_when_nothing_given() {
        let result = generation_overrides(None, None, None, None, None).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn generation_overrides_rejects_zero_max_output_tokens() {
        let err = generation_overrides(None, None, None, Some(0), None)
            .expect_err("expected an error for max_output_tokens: 0");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_rejects_zero_thinking_budget_tokens() {
        let err = generation_overrides(None, None, None, None, Some(0))
            .expect_err("expected an error for thinking_budget_tokens: 0");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_builds_params_from_new_fields() {
        let params = generation_overrides(None, None, None, Some(512), Some(1024))
            .expect("should not error")
            .expect("should build Some(GenerationParams)");
        assert_eq!(params.max_output_tokens, Some(512));
        assert_eq!(params.thinking_budget_tokens, Some(1024));
        assert_eq!(params.temperature, None);
        assert_eq!(params.reasoning_effort, None);
    }

    #[test]
    fn generation_overrides_rejects_temperature_when_unsupported() {
        let model = fixture_model(false, true, true);
        let err = generation_overrides(Some(&model), Some(0.7), None, None, None)
            .expect_err("expected temperature to be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_rejects_reasoning_effort_when_unsupported() {
        let model = fixture_model(true, false, true);
        let err = generation_overrides(Some(&model), None, Some("high"), None, None)
            .expect_err("expected reasoning_effort to be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_rejects_thinking_budget_when_unsupported() {
        let model = fixture_model(true, true, false);
        let err = generation_overrides(Some(&model), None, None, None, Some(1024))
            .expect_err("expected thinking_budget_tokens to be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_allows_supported_knobs() {
        let model = fixture_model(true, true, true);
        let params =
            generation_overrides(Some(&model), Some(0.7), Some("high"), Some(512), Some(1024))
                .expect("should not error")
                .expect("should build Some(GenerationParams)");
        assert_eq!(params.temperature, Some(0.7));
        assert_eq!(params.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(params.max_output_tokens, Some(512));
        assert_eq!(params.thinking_budget_tokens, Some(1024));
    }

    #[test]
    fn generation_overrides_skips_gating_when_model_is_none() {
        let params = generation_overrides(None, Some(0.7), Some("high"), None, Some(1024))
            .expect("should not error when no model is resolved")
            .expect("should build Some(GenerationParams)");
        assert_eq!(params.temperature, Some(0.7));
        assert_eq!(params.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(params.thinking_budget_tokens, Some(1024));
    }

    #[test]
    fn carry_forward_generation_preserves_existing_overrides_across_switch() {
        let model = fixture_model(true, true, true);
        let params =
            carry_forward_generation(&model, None, Some(0.7), Some("high"), None, Some(1024))
                .expect("existing overrides should survive the switch");
        assert_eq!(params.temperature, Some(0.7));
        assert_eq!(params.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(params.thinking_budget_tokens, Some(1024));
    }

    #[test]
    fn carry_forward_generation_drops_knobs_the_new_model_does_not_support() {
        let model = fixture_model(false, false, false);
        let params =
            carry_forward_generation(&model, None, Some(0.7), Some("high"), Some(512), Some(1024));
        // max_output_tokens has no capability gate (#53), so it survives even
        // though the temperature/reasoning_effort/thinking knobs get dropped.
        let params = params.expect("max_output_tokens alone should still produce Some");
        assert_eq!(params.temperature, None);
        assert_eq!(params.reasoning_effort, None);
        assert_eq!(params.thinking_budget_tokens, None);
        assert_eq!(params.max_output_tokens, Some(512));
    }

    #[test]
    fn carry_forward_generation_none_when_nothing_survives() {
        let model = fixture_model(false, false, false);
        assert!(
            carry_forward_generation(&model, None, Some(0.7), Some("high"), None, Some(1024))
                .is_none()
        );
    }

    #[test]
    fn carry_forward_generation_explicit_value_wins_over_existing() {
        let model = fixture_model(true, true, true);
        let explicit = GenerationParams {
            temperature: Some(0.2),
            ..Default::default()
        };
        let params = carry_forward_generation(&model, Some(explicit), Some(0.9), None, None, None)
            .expect("should build Some(GenerationParams)");
        assert_eq!(params.temperature, Some(0.2));
    }

    #[test]
    fn generation_after_model_switch_passes_explicit_through_when_model_unchanged() {
        let result = generation_after_model_switch(
            &None,
            &None,
            Some(GenerationParams::default()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(result, Some(GenerationParams::default()));
    }

    #[test]
    fn generation_after_model_switch_carries_forward_on_model_change() {
        let model = fixture_model(true, true, true);
        let provider = llm_provider::Model {
            id: 1,
            label: "fixture-provider".to_string(),
            kind: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            concurrency: None,
            rpm: None,
            created_at: chrono::Utc::now().fixed_offset(),
        };
        let model_changed = Some((model.clone(), provider));
        let result = generation_after_model_switch(
            &model_changed,
            &Some(model),
            None,
            Some(0.7),
            None,
            None,
            None,
        )
        .expect("existing temperature should carry forward");
        assert_eq!(result.temperature, Some(0.7));
    }
}
