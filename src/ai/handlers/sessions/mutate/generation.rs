//! Generation-override validation + `GenerationParams` construction for
//! `create`/`update` (#42). Split out of `mutate.rs` to keep that file under
//! the workspace's 400-line cap.

use entanglement_core::{GenerationParams, ReasoningEffort};

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
pub(super) fn generation_overrides(
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = generation_overrides(None, None, None, None).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn generation_overrides_rejects_zero_max_output_tokens() {
        let err = generation_overrides(None, None, Some(0), None)
            .expect_err("expected an error for max_output_tokens: 0");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_rejects_zero_thinking_budget_tokens() {
        let err = generation_overrides(None, None, None, Some(0))
            .expect_err("expected an error for thinking_budget_tokens: 0");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn generation_overrides_builds_params_from_new_fields() {
        let params = generation_overrides(None, None, Some(512), Some(1024))
            .expect("should not error")
            .expect("should build Some(GenerationParams)");
        assert_eq!(params.max_output_tokens, Some(512));
        assert_eq!(params.thinking_budget_tokens, Some(1024));
        assert_eq!(params.temperature, None);
        assert_eq!(params.reasoning_effort, None);
    }
}
