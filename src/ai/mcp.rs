//! `SiteMcp` — the engine's per-user MCP tool provider, built on
//! `entanglement_runtime::mcp::HttpClient` (embedding.md §6). Same shape as
//! before: discover a user's enabled `user_mcp_servers` rows, connect via
//! streamable HTTP, and namespace each remote tool as `"{server}__{tool}"`.
//! 60s TTL cache, keyed by user id (the old cache was keyed by DB session id;
//! this one by user id directly, since the new engine's `SessionId` already
//! encodes the user — see `crate::ai::engine::user_id_from_session`).
//!
//! ## The static-registry limitation (read before wiring live traffic)
//!
//! `entanglement_runtime::tool_runner::spawn_tool_executor_with_policy` takes
//! its `ToolRegistry` **by value at spawn time** — unlike `profiles` (an
//! `Arc<RwLock<..>>` a live watcher can swap), there is no live-reload seam
//! for the dispatch registry in entanglement-runtime 0.1.0. So `engine.rs`
//! seeds [`McpRoutedTool`] entries once, for every `"{server}__{tool}"`
//! identity known across *all* users at `SiteEngine::spawn` time
//! (`known_tool_names`); a server/tool a user adds afterward is advertised
//! correctly per-session (the `tool_spec_resolver` cache is refreshed live),
//! but calling it will report "not enabled for this user" until the *engine
//! process itself* restarts and re-seeds. Acceptable for this phase (nothing
//! is wired into live traffic yet); the follow-up phase should decide whether
//! that's worth a periodic executor respawn or is fine as a documented
//! restart-to-pick-up-new-servers limitation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use async_trait::async_trait;
use dashmap::DashMap;
use entanglement_core::{SessionId, ToolSpec};
use entanglement_provider::ContentPart;
use entanglement_runtime::Tool;
use entanglement_runtime::mcp::{HttpClient, McpClient};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use std::borrow::Cow;

use crate::ai::engine::user_id_from_session;
use crate::entity::user_mcp_server;
use crate::repo::tokens;

const CACHE_TTL: Duration = Duration::from_secs(60);

struct McpRoute {
    client: Arc<McpClient>,
    remote_tool: String,
    description: String,
    schema: Value,
}

struct UserCacheEntry {
    built_at: Instant,
    routes: Arc<HashMap<String, McpRoute>>,
}

pub struct SiteMcp {
    db: DatabaseConnection,
    cache: DashMap<i32, UserCacheEntry>,
}

impl SiteMcp {
    pub fn new(db: DatabaseConnection) -> Arc<Self> {
        Arc::new(SiteMcp {
            db,
            cache: DashMap::new(),
        })
    }

    /// Drop `user_id`'s cached routes — call after the next phase's
    /// user-MCP-server CRUD handlers change a row out from under this cache.
    pub fn invalidate_user(&self, user_id: i32) {
        self.cache.remove(&user_id);
    }

    async fn routes_for_user(
        &self,
        user_id: i32,
    ) -> anyhow::Result<Arc<HashMap<String, McpRoute>>> {
        if let Some(entry) = self.cache.get(&user_id)
            && entry.built_at.elapsed() < CACHE_TTL
        {
            return Ok(entry.routes.clone());
        }

        let rows = user_mcp_server::Entity::find()
            .filter(user_mcp_server::Column::UserId.eq(user_id))
            .filter(user_mcp_server::Column::Enabled.eq(true))
            .all(&self.db)
            .await
            .context("loading user_mcp_servers")?;

        let mut routes = HashMap::new();
        for row in rows {
            let mut headers = parse_headers_json(&row.headers);
            if row.forward_user_token
                && let Some(nonce) = forwardable_token(&self.db, user_id).await
            {
                headers.insert("Authorization".to_string(), format!("Bearer {nonce}"));
            }
            let client = match HttpClient::connect(&row.name, &row.url, &headers).await {
                Ok(c) => Arc::new(McpClient::Http(c)),
                Err(e) => {
                    tracing::warn!(server = %row.name, error = %e, "failed to connect to user MCP server");
                    continue;
                }
            };
            let defs = match client.list_tools().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(server = %row.name, error = %e, "failed to list tools from user MCP server");
                    continue;
                }
            };
            for def in defs {
                let prefixed = format!("{}__{}", row.name, def.name);
                routes.insert(
                    prefixed,
                    McpRoute {
                        client: client.clone(),
                        remote_tool: def.name,
                        description: def.description,
                        schema: def.input_schema,
                    },
                );
            }
        }

        let routes = Arc::new(routes);
        self.cache.insert(
            user_id,
            UserCacheEntry {
                built_at: Instant::now(),
                routes: routes.clone(),
            },
        );
        Ok(routes)
    }

    /// Tool specs currently visible to `user_id` — what `engine.rs`'s
    /// per-session `tool_spec_resolver` cache is refreshed from.
    pub async fn tool_specs_for_user(&self, user_id: i32) -> Vec<ToolSpec> {
        match self.routes_for_user(user_id).await {
            Ok(routes) => routes
                .iter()
                .map(|(name, route)| {
                    ToolSpec::with_schema(
                        name.clone(),
                        route.description.clone(),
                        route.schema.clone(),
                    )
                })
                .collect(),
            Err(e) => {
                tracing::warn!(user_id, error = %e, "failed to discover MCP tool specs");
                Vec::new()
            }
        }
    }

    /// Every `"{server}__{tool}"` identity known across every user with at
    /// least one enabled MCP server — seeded once into the static
    /// `ToolRegistry` at `SiteEngine::spawn` time (see the module doc's
    /// static-registry limitation).
    pub async fn known_tool_names(&self, db: &DatabaseConnection) -> Vec<String> {
        let user_ids: Vec<i32> = match user_mcp_server::Entity::find()
            .filter(user_mcp_server::Column::Enabled.eq(true))
            .all(db)
            .await
        {
            Ok(rows) => {
                let mut ids: Vec<i32> = rows.iter().map(|r| r.user_id).collect();
                ids.sort_unstable();
                ids.dedup();
                ids
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to list user_mcp_servers for tool seeding");
                return Vec::new();
            }
        };
        let mut names = std::collections::HashSet::new();
        for user_id in user_ids {
            if let Ok(routes) = self.routes_for_user(user_id).await {
                names.extend(routes.keys().cloned());
            }
        }
        names.into_iter().collect()
    }
}

fn parse_headers_json(raw: &serde_json::Value) -> HashMap<String, String> {
    let Some(obj) = raw.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

/// A durable bearer token to forward as this user's identity to their own MCP
/// server. Unlike the old per-request flow (which forwarded the caller's live
/// session cookie), a tool call here runs deep inside the engine executor
/// with no HTTP request in scope — so this uses one of the user's *service*
/// tokens (no expiry) instead. `None` (and a forwarded-token-less connection)
/// if the user has none; the server then sees an unauthenticated request,
/// same as today when `forward_user_token` is set but no token is available.
async fn forwardable_token(db: &DatabaseConnection, user_id: i32) -> Option<String> {
    match tokens::list_service_tokens(db, user_id).await {
        Ok(rows) => rows.into_iter().next().map(|t| t.nonce),
        Err(e) => {
            tracing::warn!(user_id, error = %e, "failed to load service token for MCP forwarding");
            None
        }
    }
}

/// A single registered tool identity (`"{server}__{tool}"`) that routes to
/// whichever concrete MCP server *the calling session's user* has configured
/// under that name — see the module doc for why the routing (not the tool
/// object itself) is what's session-aware.
pub struct McpRoutedTool {
    name: String,
    mcp: Arc<SiteMcp>,
}

impl McpRoutedTool {
    pub fn new(name: String, mcp: Arc<SiteMcp>) -> Self {
        McpRoutedTool { name, mcp }
    }
}

#[async_trait]
impl Tool for McpRoutedTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Owned(self.name.clone())
    }

    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("mcp tools are session-scoped; use run_for_session")
    }

    async fn run_for_session(
        &self,
        session: &SessionId,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let user_id = user_id_from_session(session)?;
        let routes = self.mcp.routes_for_user(user_id).await?;
        let route = routes
            .get(&self.name)
            .ok_or_else(|| anyhow::anyhow!("tool `{}` is not enabled for this user", self.name))?;
        let args: Value = if input.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(input).context("invalid tool arguments")?
        };
        let result = route
            .client
            .call_tool(&route.remote_tool, args)
            .await
            .context("mcp tool call failed")?;
        let text = match &result {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Ok(vec![ContentPart::text(text)])
    }
}
