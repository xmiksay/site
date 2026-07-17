//! `SiteCatalog` — the engine's model catalog, building
//! `entanglement_provider::LlmFactory`/`ModelResolver` closures. Hydrated from
//! the `llm_providers`/`llm_models` tables; `refresh()` re-reads them (call
//! after provider/model CRUD in the handlers).
//!
//! **`ModelResolver` keying convention** (documented here for the follow-up
//! phase that mints `InMsg::SetModel`): `provider` = `llm_providers.label`
//! (unique display name), `model` = `llm_models.id` as a decimal string. Our
//! own primary keys are the only unambiguous handle — two provider rows of
//! the same `kind` (e.g. two `anthropic` connections) or two models sharing a
//! wire id string are both legal, so keying by kind/wire-id would collide.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use entanglement_provider::{
    GEMINI_BASE, HttpClient, LlmFactory, ModelResolver, OLLAMA_BASE, ResolvedModel,
    anthropic_factory, gemini_factory, openai_factory,
};
use parking_lot::RwLock;
use sea_orm::{DatabaseConnection, EntityTrait};

use crate::entity::{llm_model, llm_provider};

/// One usable (provider row, model row) pairing, with its ready-to-call LLM
/// factory closure baked in.
#[derive(Clone)]
pub struct CatalogModel {
    pub model_id: i32,
    pub provider_id: i32,
    pub provider_label: String,
    pub kind: String,
    pub wire_model: String,
    pub is_default: bool,
    pub llm_factory: LlmFactory,
}

#[derive(Default)]
struct CatalogInner {
    by_model_id: HashMap<i32, CatalogModel>,
    default_model_id: Option<i32>,
}

pub struct SiteCatalog {
    db: DatabaseConnection,
    http: HttpClient,
    /// `parking_lot::RwLock`, not `std::sync`: a panic while this is locked
    /// must not poison it and fail-closed every later `model_by_id`/
    /// `default_llm_factory` lookup for every session (issue #28).
    inner: RwLock<CatalogInner>,
}

impl SiteCatalog {
    /// Load the catalog from the DB. Returns an `Arc` since `engine.rs` shares
    /// it between `EngineConfig.model_resolver` and any future admin surface.
    pub async fn load(db: DatabaseConnection) -> anyhow::Result<Arc<Self>> {
        let catalog = Arc::new(SiteCatalog {
            db,
            http: HttpClient::new(),
            inner: RwLock::new(CatalogInner::default()),
        });
        catalog.refresh().await?;
        Ok(catalog)
    }

    /// Re-read `llm_providers`/`llm_models` and rebuild every factory closure.
    /// Call after provider/model CRUD so live sessions pick up the change on
    /// their next `SetModel`/new-session resolve.
    pub async fn refresh(&self) -> anyhow::Result<()> {
        let providers = llm_provider::Entity::find()
            .all(&self.db)
            .await
            .context("loading llm_providers")?;
        let models = llm_model::Entity::find()
            .all(&self.db)
            .await
            .context("loading llm_models")?;

        let mut by_model_id = HashMap::with_capacity(models.len());
        let mut default_model_id = None;
        for model in &models {
            let Some(provider) = providers.iter().find(|p| p.id == model.provider_id) else {
                tracing::warn!(
                    model_id = model.id,
                    "llm_models row has no matching provider; skipping"
                );
                continue;
            };
            let llm_factory = match build_factory(provider, &model.model, &self.http) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(model_id = model.id, error = %e, "skipping unbuildable model");
                    continue;
                }
            };
            if model.is_default {
                default_model_id = Some(model.id);
            }
            by_model_id.insert(
                model.id,
                CatalogModel {
                    model_id: model.id,
                    provider_id: provider.id,
                    provider_label: provider.label.clone(),
                    kind: provider.kind.clone(),
                    wire_model: model.model.clone(),
                    is_default: model.is_default,
                    llm_factory,
                },
            );
        }
        // First model row is the fallback default if none is flagged, mirroring
        // `ProviderRegistry::resolve_default`.
        if default_model_id.is_none() {
            default_model_id = models.first().map(|m| m.id);
        }
        *self.inner.write() = CatalogInner {
            by_model_id,
            default_model_id,
        };
        Ok(())
    }

    pub fn model_by_id(&self, model_id: i32) -> Option<CatalogModel> {
        self.inner.read().by_model_id.get(&model_id).cloned()
    }

    pub fn default_model(&self) -> Option<CatalogModel> {
        let inner = self.inner.read();
        inner
            .default_model_id
            .and_then(|id| inner.by_model_id.get(&id).cloned())
    }

    /// The factory `EngineConfig.llm_factory` should use before any
    /// per-session `SetModel` — the default model's factory, or `EchoLlm` if
    /// nothing is configured yet (matching `EngineConfig::default()`'s own
    /// fallback, so an empty catalog degrades gracefully instead of panicking).
    pub fn default_llm_factory(&self) -> LlmFactory {
        match self.default_model() {
            Some(m) => m.llm_factory,
            None => Arc::new(|| Box::new(entanglement_provider::EchoLlm)),
        }
    }

    /// Build the `ModelResolver` closure for `EngineConfig.model_resolver`
    /// (live `InMsg::SetModel` support). See the module doc for the
    /// `provider`/`model` keying convention.
    pub fn model_resolver(self: &Arc<Self>) -> ModelResolver {
        let catalog = self.clone();
        Arc::new(
            move |provider: &str, model: &str| -> Result<ResolvedModel, String> {
                let model_id: i32 = model
                    .parse()
                    .map_err(|_| format!("model `{model}` is not a valid model id"))?;
                let found = catalog
                    .model_by_id(model_id)
                    .ok_or_else(|| format!("model id {model_id} not found"))?;
                if found.provider_label != provider {
                    return Err(format!(
                        "model id {model_id} belongs to provider `{}`, not `{provider}`",
                        found.provider_label
                    ));
                }
                Ok(ResolvedModel {
                    provider: found.provider_label,
                    model: found.wire_model,
                    llm_factory: found.llm_factory,
                    generation: None,
                    context_window: None,
                })
            },
        )
    }
}

/// Build the `LlmFactory` for one provider row, dispatching on `kind`.
fn build_factory(
    provider: &llm_provider::Model,
    default_model: &str,
    http: &HttpClient,
) -> anyhow::Result<LlmFactory> {
    match provider.kind.as_str() {
        "ollama" => Ok(openai_factory(
            ollama_base_url(provider),
            None,
            default_model,
            None,
            None,
            http.clone(),
        )),
        "anthropic" => {
            let api_key = provider.api_key.clone().ok_or_else(|| {
                anyhow::anyhow!("anthropic provider '{}' has no api_key", provider.label)
            })?;
            Ok(anthropic_factory(
                api_key,
                default_model,
                None,
                None,
                http.clone(),
            ))
        }
        "gemini" => {
            let api_key = provider.api_key.clone().ok_or_else(|| {
                anyhow::anyhow!("gemini provider '{}' has no api_key", provider.label)
            })?;
            Ok(gemini_factory(
                GEMINI_BASE,
                api_key,
                default_model,
                None,
                http.clone(),
            ))
        }
        other => anyhow::bail!("provider kind not supported: {other}"),
    }
}

/// Effective Ollama base URL for `provider`'s row: its own `base_url` unless
/// unset/blank, else the default local endpoint. Split out from
/// `build_factory` so the fallback is unit-testable without a network call
/// (the resulting `LlmFactory` closure is opaque — there's no other way to
/// observe which URL it captured).
fn ollama_base_url(provider: &llm_provider::Model) -> String {
    provider
        .base_url
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| OLLAMA_BASE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact lock type `SiteCatalog.inner` uses. A `std::sync::RwLock`
    /// would poison here — this proves `parking_lot::RwLock` doesn't, so one
    /// panicking `refresh()` call can't fail-closed every later
    /// `model_by_id`/`default_model` lookup for every session (issue #28).
    #[test]
    fn panicking_while_holding_the_write_lock_does_not_poison_it() {
        let lock = Arc::new(RwLock::new(CatalogInner::default()));
        let panicking = lock.clone();

        let result = std::thread::spawn(move || {
            let _guard = panicking.write();
            panic!("simulated panic mid-refresh");
        })
        .join();
        assert!(result.is_err(), "the spawned thread should have panicked");

        // A `std::sync::RwLock` would return `Err(Poisoned)` here instead.
        let inner = lock.read();
        assert!(inner.by_model_id.is_empty());
        assert!(inner.default_model_id.is_none());
    }

    fn provider(kind: &str, api_key: Option<&str>, base_url: Option<&str>) -> llm_provider::Model {
        llm_provider::Model {
            id: 1,
            label: "test-provider".to_string(),
            kind: kind.to_string(),
            api_key: api_key.map(str::to_string),
            base_url: base_url.map(str::to_string),
            created_at: chrono::Utc::now().fixed_offset(),
        }
    }

    #[test]
    fn ollama_without_base_url_falls_back_to_default() {
        let p = provider("ollama", None, None);
        assert_eq!(ollama_base_url(&p), OLLAMA_BASE);
        assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
    }

    #[test]
    fn ollama_with_blank_base_url_falls_back_to_default() {
        let p = provider("ollama", None, Some(""));
        assert_eq!(ollama_base_url(&p), OLLAMA_BASE);
    }

    #[test]
    fn ollama_with_base_url_uses_it() {
        let p = provider("ollama", None, Some("http://example.internal:1234/v1"));
        assert_eq!(ollama_base_url(&p), "http://example.internal:1234/v1");
    }

    #[test]
    fn anthropic_without_api_key_errs() {
        let p = provider("anthropic", None, None);
        let err = build_factory(&p, "model", &HttpClient::new())
            .err()
            .expect("expected build_factory to fail");
        assert!(err.to_string().contains("no api_key"));
    }

    #[test]
    fn anthropic_with_api_key_builds_ok() {
        let p = provider("anthropic", Some("key"), None);
        assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
    }

    #[test]
    fn gemini_without_api_key_errs() {
        let p = provider("gemini", None, None);
        let err = build_factory(&p, "model", &HttpClient::new())
            .err()
            .expect("expected build_factory to fail");
        assert!(err.to_string().contains("no api_key"));
    }

    #[test]
    fn gemini_with_api_key_builds_ok() {
        let p = provider("gemini", Some("key"), None);
        assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
    }

    #[test]
    fn unsupported_kind_errs_naming_it() {
        let p = provider("mystery", None, None);
        let err = build_factory(&p, "model", &HttpClient::new())
            .err()
            .expect("expected build_factory to fail");
        assert!(err.to_string().contains("mystery"));
    }
}
