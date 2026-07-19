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
    GEMINI_BASE, GenerationResolver, HttpClient, LlmFactory, ModelResolver, OLLAMA_BASE,
    ResolvedModel, anthropic_factory, gemini_factory, openai_factory,
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
    /// The model's real context window in tokens (#40), from `llm_models`.
    pub context_window: Option<usize>,
    /// Effective per-endpoint in-flight request cap threaded into
    /// `llm_factory` (ADR-0111), from `llm_providers.concurrency`. `None` ⇒
    /// `entanglement_provider`'s own client default. Exposed for
    /// introspection/tests — the value is already baked into the closure.
    pub concurrency: Option<usize>,
    /// Effective per-endpoint requests-per-minute budget threaded into
    /// `llm_factory`, from `llm_providers.rpm`. `None` ⇒ the client default.
    pub rpm: Option<u32>,
    pub llm_factory: LlmFactory,
}

#[derive(Default)]
struct CatalogInner {
    by_model_id: HashMap<i32, CatalogModel>,
    default_model_id: Option<i32>,
}

pub struct SiteCatalog {
    db: DatabaseConnection,
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
            inner: RwLock::new(CatalogInner::default()),
        });
        catalog.refresh().await?;
        Ok(catalog)
    }

    /// Re-read `llm_providers`/`llm_models` and rebuild every factory
    /// closure, including a **fresh** `HttpClient` to build them against.
    /// `entanglement_provider::HttpClient`'s per-endpoint rpm/concurrency
    /// state is created lazily on an endpoint's *first* request and then
    /// locked in for that `HttpClient`'s lifetime (see its own `endpoint()`
    /// doc: "Only the first caller for a key sets the bucket size"). Reusing
    /// one long-lived `HttpClient` across every `refresh()` would mean an
    /// admin's `concurrency`/`rpm` edit (#41/ADR-0111) never takes effect
    /// once *any* session had already hit that provider's endpoint —
    /// rebuilding here costs nothing until first use (no eager connections)
    /// and only affects *new* sessions/turns resolved after this refresh,
    /// matching this method's existing "next new session" contract. Each
    /// `LlmFactory` closure clones this `http` into itself (`build_factory`),
    /// which is what keeps its `Arc`-shared pool alive — nothing here needs
    /// to hold onto `http` past this function returning.
    ///
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

        let http = HttpClient::new();
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
            let llm_factory = match build_factory(provider, &model.model, &http) {
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
                    context_window: model.context_window.and_then(|w| usize::try_from(w).ok()),
                    concurrency: positive_usize(provider.concurrency),
                    rpm: positive_u32(provider.rpm),
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

    /// A default factory that defers to the *current* default at call time,
    /// so `refresh()` (provider/model CRUD) takes effect for un-pinned/resumed
    /// sessions without a server restart. Unlike [`default_llm_factory`][Self::
    /// default_llm_factory] — whose result the engine would otherwise freeze
    /// into `EngineConfig.llm_factory` at spawn — this closure re-reads the
    /// catalog every time the engine builds an LLM for a session that has no
    /// `SetModel` pin yet (a fresh session's first turn, a `/compact` fork's
    /// seed, a resumed session before replay re-pins it).
    pub fn dynamic_default_factory(self: &Arc<Self>) -> LlmFactory {
        let catalog = self.clone();
        Arc::new(move || (catalog.default_llm_factory())())
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
                    context_window: found.context_window,
                })
            },
        )
    }

    /// Build the `GenerationResolver` closure for
    /// `EngineConfig.generation_resolver` (per-*profile* persisted generation
    /// overrides, ADR-0094 — the generation-parameter analogue of
    /// `AgentProfile::model_pin`). This site has no per-profile generation
    /// config store: unlike the model pin, which `engine/profiles.rs` bakes
    /// straight into each `AgentProfile`, generation knobs (temperature,
    /// reasoning effort) are set live per-*session* via `InMsg::SetGeneration`
    /// (`handlers/sessions`, #42), not pinned per-profile. Always returns
    /// `None`, wiring the seam for parity with [`model_resolver`][Self::
    /// model_resolver] without inventing an admin surface nothing populates.
    pub fn generation_resolver(self: &Arc<Self>) -> GenerationResolver {
        Arc::new(|_profile: &str| None)
    }
}

/// Build the `LlmFactory` for one provider row, dispatching on `kind`.
///
/// `provider.rpm`/`.concurrency` (ADR-0111) are threaded straight into the
/// factory so the client's per-endpoint pacing gate and in-flight permit are
/// sized from the DB row instead of the library's process-wide defaults —
/// this is what serializes many spawned sub-agents against one provider's
/// real limits instead of 429-storming it.
fn build_factory(
    provider: &llm_provider::Model,
    default_model: &str,
    http: &HttpClient,
) -> anyhow::Result<LlmFactory> {
    let rpm = positive_u32(provider.rpm);
    let concurrency = positive_usize(provider.concurrency);
    match provider.kind.as_str() {
        "ollama" => Ok(openai_factory(
            ollama_base_url(provider),
            None,
            default_model,
            rpm,
            concurrency,
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
                rpm,
                concurrency,
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
                rpm,
                concurrency,
                http.clone(),
            ))
        }
        "openai" => {
            let base_url = provider
                .base_url
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("openai provider '{}' has no base_url", provider.label)
                })?;
            Ok(openai_factory(
                base_url,
                provider.api_key.clone().filter(|s| !s.is_empty()),
                default_model,
                rpm,
                concurrency,
                None,
                http.clone(),
            ))
        }
        other => anyhow::bail!("provider kind not supported: {other}"),
    }
}

/// A DB-stored budget clamped to the factories' expected type. A non-positive
/// value is treated as "unset" (falls back to the client's own default)
/// rather than panicking on the cast or silently passing a zero-sized budget.
fn positive_u32(v: Option<i32>) -> Option<u32> {
    v.and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

/// See [`positive_u32`]; same clamp for the `usize` concurrency cap.
fn positive_usize(v: Option<i32>) -> Option<usize> {
    v.and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0)
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
impl SiteCatalog {
    /// Build a catalog with a pre-seeded `inner` and a disconnected DB — for
    /// unit tests that exercise the in-memory lookup/factory paths (which never
    /// touch `db`) without a live Postgres.
    fn new_for_test(inner: CatalogInner) -> Self {
        SiteCatalog {
            db: DatabaseConnection::default(),
            inner: RwLock::new(inner),
        }
    }

    /// Rewrite the default model id the way `refresh()` would on an admin edit.
    fn set_default_for_test(&self, id: Option<i32>) {
        self.inner.write().default_model_id = id;
    }
}

#[cfg(test)]
mod tests;
