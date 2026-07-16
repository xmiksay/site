use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait, ModelTrait};
use tokio::sync::Mutex;

use crate::ai::llm::{self, ChatRequest, ChatResponse, LlmProvider, ProviderError};
use crate::entity::{llm_model, llm_provider};

/// Wrapper that serializes every `chat()` call to a given provider through a
/// Tokio mutex. There is one mutex per provider row, shared by every caller,
/// so a single backend connection is invoked at most once at a time.
struct SerializedProvider {
    inner: Arc<dyn LlmProvider>,
    lock: Arc<Mutex<()>>,
    name: &'static str,
    default_model: String,
}

#[async_trait]
impl LlmProvider for SerializedProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let _guard = self.lock.lock().await;
        self.inner.chat(req).await
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("model not found: id={0}")]
    ModelNotFound(i32),
    #[error("provider not found for model id={0}")]
    ProviderNotFound(i32),
    #[error("no models configured — add a provider and a model first")]
    Empty,
    #[error("provider kind not supported: {0}")]
    UnsupportedKind(String),
    #[error("provider configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
}

pub struct ResolvedModel {
    pub provider: Arc<dyn LlmProvider>,
    pub kind: String,
    pub model: String,
    pub model_id: i32,
    pub provider_id: i32,
}

pub struct ProviderRegistry {
    db: DatabaseConnection,
    /// Cache keyed by provider row id (so multiple models that share a
    /// provider also share the connection mutex).
    cache: RwLock<HashMap<i32, Arc<dyn LlmProvider>>>,
}

impl ProviderRegistry {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Resolve a model row by id, follow its provider FK, build (or fetch the
    /// cached) backend client, and return the wire model identifier alongside.
    pub async fn resolve(&self, model_id: i32) -> Result<ResolvedModel, RegistryError> {
        let model_row = llm_model::Entity::find_by_id(model_id)
            .one(&self.db)
            .await?
            .ok_or(RegistryError::ModelNotFound(model_id))?;
        let provider_row = model_row
            .find_related(llm_provider::Entity)
            .one(&self.db)
            .await?
            .ok_or(RegistryError::ProviderNotFound(model_id))?;
        let provider = self.get_or_build(&provider_row, &model_row.model)?;
        Ok(ResolvedModel {
            provider,
            kind: provider_row.kind,
            model: model_row.model,
            model_id: model_row.id,
            provider_id: provider_row.id,
        })
    }

    /// Resolve the default model — falls back to the first model row if no
    /// row is flagged default.
    pub async fn resolve_default(&self) -> Result<ResolvedModel, RegistryError> {
        let rows = llm_model::Entity::find().all(&self.db).await?;
        if rows.is_empty() {
            return Err(RegistryError::Empty);
        }
        let row = rows
            .iter()
            .find(|r| r.is_default)
            .or_else(|| rows.first())
            .cloned()
            .unwrap();
        self.resolve(row.id).await
    }

    pub fn invalidate(&self, provider_id: i32) {
        self.cache.write().unwrap().remove(&provider_id);
    }

    fn get_or_build(
        &self,
        row: &llm_provider::Model,
        model_name: &str,
    ) -> Result<Arc<dyn LlmProvider>, RegistryError> {
        if let Some(p) = self.cache.read().unwrap().get(&row.id) {
            return Ok(p.clone());
        }
        let built = self.build(row, model_name)?;
        self.cache.write().unwrap().insert(row.id, built.clone());
        Ok(built)
    }

    fn build(
        &self,
        row: &llm_provider::Model,
        default_model: &str,
    ) -> Result<Arc<dyn LlmProvider>, RegistryError> {
        let (inner, name): (Arc<dyn LlmProvider>, &'static str) = match row.kind.as_str() {
            "ollama" => {
                let url = row.base_url.clone().ok_or_else(|| {
                    RegistryError::Config(format!(
                        "ollama provider '{}' has no base_url",
                        row.label
                    ))
                })?;
                (
                    Arc::new(llm::ollama::OllamaProvider::new(url, default_model.into())),
                    "ollama",
                )
            }
            "anthropic" => {
                let api_key = row.api_key.clone().ok_or_else(|| {
                    RegistryError::Config(format!(
                        "anthropic provider '{}' has no api_key",
                        row.label
                    ))
                })?;
                (
                    Arc::new(llm::anthropic::AnthropicProvider::new(
                        api_key,
                        default_model.into(),
                    )),
                    "anthropic",
                )
            }
            "gemini" => {
                let api_key = row.api_key.clone().ok_or_else(|| {
                    RegistryError::Config(format!("gemini provider '{}' has no api_key", row.label))
                })?;
                (
                    Arc::new(llm::gemini::GeminiProvider::new(
                        api_key,
                        default_model.into(),
                    )),
                    "gemini",
                )
            }
            other => return Err(RegistryError::UnsupportedKind(other.to_string())),
        };
        Ok(Arc::new(SerializedProvider {
            inner,
            lock: Arc::new(Mutex::new(())),
            name,
            default_model: default_model.to_string(),
        }))
    }
}
