//! `SiteMcp` — the engine's per-user MCP tool provider, built on
//! `entanglement_runtime::mcp::HttpClient` (embedding.md §6). Same shape as
//! before: discover a user's enabled `user_mcp_servers` rows, connect via
//! streamable HTTP, and namespace each remote tool as `"{server}__{tool}"`.
//! 60s TTL cache, keyed by user id (the old cache was keyed by DB session id;
//! this one by user id directly, since the new engine's `SessionId` already
//! encodes the user — see `crate::ai::engine::user_id_from_session`).
//!
//! ## Live registration (issue #38)
//!
//! 0.3's `entanglement_runtime::tool_runner::spawn_tool_executor_with_policy`
//! takes its `ToolRegistry` wrapped in a `SharedRegistry`
//! (`Arc<RwLock<ToolRegistry>>`) that can be mutated in place while the
//! executor runs — `SiteEngine::spawn` wraps it once and hands `SiteMcp` the
//! same handle. Every time [`routes_for_user`][SiteMcp::routes_for_user]
//! (re)builds a user's routes — a fresh connect, whether from a cold cache or
//! a TTL expiry — it registers each newly discovered `"{server}__{tool}"`
//! identity into that registry (`register_routes`), so a server a user adds
//! becomes dispatchable as soon as its routes are next resolved, with no
//! executor restart or periodic rebuild needed.
//! [`invalidate_user`][SiteMcp::invalidate_user] is the deregistering
//! counterpart: it drops a user's cached routes and, for each identity no
//! *other* currently-cached user still has, unregisters it too — call it
//! after mcp-server CRUD (`ai::handlers::mcp_servers`) changes a row out from
//! under the cache.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use async_trait::async_trait;
use dashmap::DashMap;
use entanglement_core::{SessionId, ToolSpec};
use entanglement_provider::ContentPart;
use entanglement_runtime::mcp::{HttpClient, McpClient};
use entanglement_runtime::{SharedRegistry, Tool};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use std::borrow::Cow;

use crate::ai::engine::user_id_from_session;
use crate::entity::user_mcp_server;
use crate::repo::tokens;

const CACHE_TTL: Duration = Duration::from_secs(60);

/// Ceiling on one remote MCP server's `connect()` handshake (TCP connect +
/// `initialize` + `notifications/initialized`). Without this, a single
/// unresponsive server can stall `routes_for_user` for as long as the
/// underlying HTTP client's own per-request timeout (up to ~2 minutes across
/// the handshake's two round-trips). See issue #28.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `HttpClient::connect`, bounded to `timeout_duration` — see [`CONNECT_TIMEOUT`]'s
/// doc. A parameter (not just the constant baked in) so a test can prove the
/// bound actually applies without waiting out the real production timeout.
async fn connect_with_timeout(
    server: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_duration: Duration,
) -> anyhow::Result<HttpClient> {
    match tokio::time::timeout(timeout_duration, HttpClient::connect(server, url, headers)).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "MCP server `{server}` timed out connecting after {}s",
            timeout_duration.as_secs_f64()
        ),
    }
}

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
    registry: SharedRegistry,
    /// Set to a weak handle on itself right after construction (the "weak
    /// self" idiom) so [`register_routes`][Self::register_routes] can hand
    /// `McpRoutedTool` the `Arc<SiteMcp>` it needs to route calls, from a
    /// `&self` method — `routes_for_user` has no owned `Arc` of its own to
    /// give it.
    self_ref: OnceLock<Weak<SiteMcp>>,
}

impl SiteMcp {
    pub fn new(db: DatabaseConnection, registry: SharedRegistry) -> Arc<Self> {
        let mcp = Arc::new(SiteMcp {
            db,
            cache: DashMap::new(),
            registry,
            self_ref: OnceLock::new(),
        });
        let _ = mcp.self_ref.set(Arc::downgrade(&mcp));
        mcp
    }

    /// Drop `user_id`'s cached routes and deregister any of its
    /// `"{server}__{tool}"` identities that no other currently-cached user
    /// still has — the live-deregistration counterpart to
    /// [`register_routes`][Self::register_routes]. Call after the mcp-server
    /// CRUD handlers change a row out from under this cache.
    pub fn invalidate_user(&self, user_id: i32) {
        let Some((_, removed)) = self.cache.remove(&user_id) else {
            return;
        };
        let mut reg = self.registry.write().unwrap();
        'names: for name in removed.routes.keys() {
            for other in self.cache.iter() {
                if other.routes.contains_key(name) {
                    continue 'names;
                }
            }
            reg.unregister(name);
        }
    }

    /// Register `routes`' `"{server}__{tool}"` identities into the shared
    /// dispatch registry (see the module doc) so each becomes callable the
    /// moment it's discovered. Upsert-only — skips a name already registered
    /// rather than replacing it, since multiple users can share an identity
    /// string and dispatch always re-resolves the concrete route per calling
    /// user anyway (`McpRoutedTool::run_for_session`).
    fn register_routes(&self, routes: &HashMap<String, McpRoute>) {
        let Some(mcp) = self.self_ref.get().and_then(Weak::upgrade) else {
            return;
        };
        let mut reg = self.registry.write().unwrap();
        for name in routes.keys() {
            if !reg.contains(name) {
                reg.register(McpRoutedTool::new(name.clone(), mcp.clone()));
            }
        }
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
            let client = match connect_with_timeout(&row.name, &row.url, &headers, CONNECT_TIMEOUT)
                .await
            {
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

        self.register_routes(&routes);

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
        _request_id: &str,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A remote MCP server that accepts the TCP connection but never answers
    /// — the boot-latency failure mode issue #28 calls out. Proves
    /// `connect_with_timeout` returns in bounded time instead of hanging for
    /// as long as the underlying HTTP client's own (much longer) per-request
    /// timeout.
    #[tokio::test]
    async fn connect_with_timeout_bounds_a_server_that_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            // Accept and hold every connection open without ever writing a
            // response — a black hole, not a refusal.
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                std::mem::forget(socket); // keep the fd open for the test's duration
            }
        });

        let bound = Duration::from_millis(200);
        let started = tokio::time::Instant::now();
        let result = connect_with_timeout(
            "black-hole",
            &format!("http://{addr}/mcp"),
            &HashMap::new(),
            bound,
        )
        .await;
        let elapsed = started.elapsed();

        assert!(
            result.is_err(),
            "a server that never answers must not connect"
        );
        assert!(
            elapsed < bound * 5,
            "connect_with_timeout took {elapsed:?}, expected roughly the {bound:?} bound"
        );
    }
}
