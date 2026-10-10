//! Row builders shared by the catalog's unit tests. Lives in its own file
//! because both `factory.rs`'s tests (provider-kind dispatch) and `tests.rs`
//! (throttle/default-model surface) need the same hand-built entity rows —
//! the second use is where the workspace DRY rule says to extract.

use crate::entity::{llm_model, llm_provider};

pub(crate) fn provider(
    kind: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> llm_provider::Model {
    provider_with_limits(kind, api_key, base_url, None, None)
}

pub(crate) fn provider_with_limits(
    kind: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    concurrency: Option<i32>,
    rpm: Option<i32>,
) -> llm_provider::Model {
    llm_provider::Model {
        id: 1,
        label: "test-provider".to_string(),
        kind: kind.to_string(),
        api_key: api_key.map(str::to_string),
        base_url: base_url.map(str::to_string),
        concurrency,
        rpm,
        created_at: chrono::Utc::now().fixed_offset(),
    }
}

/// One `llm_models` row with only the fields `choose_default_model_id` reads.
pub(crate) fn model_row(id: i32, is_default: bool) -> llm_model::Model {
    llm_model::Model {
        id,
        provider_id: 1,
        label: format!("label-{id}"),
        model: format!("model-{id}"),
        is_default,
        context_window: None,
        supports_temperature: true,
        supports_reasoning_effort: false,
        supports_thinking: false,
        supports_images: true,
        created_at: chrono::Utc::now().fixed_offset(),
    }
}
