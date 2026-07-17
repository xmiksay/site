//! Building and periodically refreshing [`SiteEngine`]'s tool-dispatch
//! registry — split out of `engine.rs` to keep that file under the 400-line
//! cap. See `mcp.rs`'s static-registry limitation doc for *why* this needs a
//! periodic rebuild at all: `entanglement_runtime::tool_runner::
//! spawn_tool_executor_with_policy` takes its `ToolRegistry` by value at
//! spawn time, with no live-reload seam for it (unlike `profiles`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;

use entanglement_core::Permission;
use entanglement_runtime::ToolRegistry;
use entanglement_runtime::hooks::Hooks;
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use entanglement_runtime::tool_runner;
use sea_orm::DatabaseConnection;

use super::SiteEngine;
use crate::ai::mcp::{McpRoutedTool, SiteMcp};
use crate::ai::tools;
use crate::routes::ws::WsHub;

/// How often the tool executor's dispatch registry is rebuilt and swapped in
/// — see [`SiteEngine::refresh_tool_registry`]. `entanglement-runtime` 0.1.0
/// has no live-reload seam for `tool_runner`'s `ToolRegistry` (unlike
/// `profiles`, already `Arc<RwLock<..>>`), so a server a user enables after
/// `spawn` is otherwise invisible to dispatch until process restart
/// (`mcp.rs`'s static-registry limitation doc, issue #28). Minutes, not
/// seconds: a full rebuild reconnects to every enabled remote MCP server, so
/// this trades "new server usable within N minutes" against not hammering
/// those servers.
pub(super) const TOOL_REGISTRY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Build a fresh registry of the built-in (non-MCP) tools — the shared first
/// half of both the initial `spawn` build and every later
/// [`SiteEngine::refresh_tool_registry`] rebuild.
pub(super) fn local_tool_registry(
    db: &DatabaseConnection,
    ws_hub: &Arc<WsHub>,
    serper_api_key: &Option<String>,
) -> ToolRegistry {
    tools::registry(Arc::new(db.clone()), ws_hub.clone(), serper_api_key.clone())
}

/// Layer every currently-known `"{server}__{tool}"` MCP identity (across every
/// user with an enabled server, see `mcp.rs`'s static-registry limitation
/// doc) onto `registry` — the shared second half of both the initial `spawn`
/// build and every later [`SiteEngine::refresh_tool_registry`] rebuild.
pub(super) async fn register_mcp_tools(
    registry: &mut ToolRegistry,
    db: &DatabaseConnection,
    mcp: &Arc<SiteMcp>,
) {
    for name in mcp.known_tool_names(db).await {
        registry.register(McpRoutedTool::new(name, mcp.clone()));
    }
}

impl SiteEngine {
    /// Rebuild the tool-dispatch registry from the current DB state (built-in
    /// tools + every user's currently-enabled MCP servers) and swap it into a
    /// freshly spawned tool executor, replacing the running one. Called
    /// periodically (see [`TOOL_REGISTRY_REFRESH_INTERVAL`]); exposed as a
    /// method rather than buried in the background task so a test — or a
    /// future "I just added a server" handler — can force an immediate
    /// refresh instead of waiting for the next tick.
    ///
    /// The old executor is aborted *before* the new one is spawned, not
    /// after: `tool_runner::spawn_tool_executor_with_policy` subscribes to
    /// `Holly`'s broadcast synchronously, so briefly running both would let a
    /// `ToolExec` racing the swap be picked up — and dispatched — by both,
    /// double-running a possibly destructive tool call. Aborting first instead
    /// only risks that same race window landing a `ToolExec` when *neither*
    /// executor is listening; core's own re-offer timer (ADR-0071) re-emits
    /// an unresolved parked call after a stretch of silence, so the new
    /// executor picks it up shortly after — delayed, never lost, and never
    /// double-run. See issue #28.
    pub async fn refresh_tool_registry(&self, db: &DatabaseConnection) {
        let mut registry = local_tool_registry(db, &self.ws_hub, &self.serper_api_key);
        register_mcp_tools(&mut registry, db, &self.mcp).await;

        let resolver: Arc<dyn PermissionResolver> = self.policy.clone();
        let grants: Arc<dyn GrantStore> = self.policy.clone();

        let mut current = self.tool_executor.lock();
        current.abort();
        *current = tool_runner::spawn_tool_executor_with_policy(
            &self.holly,
            registry,
            Arc::new(StdRwLock::new(self.profiles.clone())),
            entanglement_core::PermissionProfile::new(Permission::Allow),
            Arc::new(StdMutex::new(HashMap::new())),
            resolver,
            grants,
            Hooks::default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::config::AiConfig;

    /// DB-gated (per the repo's convention — see `tests/policy_db.rs`): a full
    /// `SiteEngine::spawn` needs a real Postgres. Proves the issue #28 fix
    /// directly — `refresh_tool_registry` actually installs a *new* executor
    /// task rather than being a no-op, which is what let a server a user
    /// enabled after boot stay permanently undispatchable before this fix.
    #[tokio::test]
    async fn refresh_tool_registry_installs_a_freshly_spawned_executor() {
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let db = sea_orm::Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        let ws_hub = Arc::new(WsHub::new());
        let ai_config = Arc::new(AiConfig::new());
        let engine = SiteEngine::spawn(db.clone(), ai_config, ws_hub, None, None)
            .await
            .expect("spawn engine");

        let before = engine.tool_executor.lock().id();
        engine.refresh_tool_registry(&db).await;
        let after = engine.tool_executor.lock().id();

        assert_ne!(
            before, after,
            "refresh_tool_registry must swap in a freshly spawned executor task"
        );
    }
}
