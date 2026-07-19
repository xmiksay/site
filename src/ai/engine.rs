//! `SiteEngine` — the single process-wide `Holly` (embedding.md §1: one
//! engine for every tenant, sessions namespaced per user) wired into
//! `state.rs` as the turn-driving stack.
//!
//! See [`session_tree`] for the `u{user_id}:{uuid}` session-id convention and
//! how a sub-agent (#17) child session resolves back to its owning user, and
//! [`profiles`] for the `researcher`/`page-writer` sub-agent profile roster.
//!
//! ## Public API for the next phase
//!
//! ```ignore
//! let engine = SiteEngine::spawn(db, ai_config, ws_hub, serper_api_key, None).await?;
//! let session = SiteEngine::session_id_for_user(user_id);
//! engine.holly.send(InMsg::prompt(session, text)).await?;
//! ```
//!
//! `ws_hub` is threaded into the built-in tool registry (`tools::registry`)
//! so a page/file/gallery/tag mutation made by the AI assistant broadcasts
//! the same `WsHub` event a REST API mutation would (issue #25) — `state.rs`
//! constructs the hub before the engine so it can be passed in here.
//!
//! `spawn`'s last parameter, `llm_factory_override`, exists solely as a test
//! seam: `state.rs`'s one production call site always passes `None` (the
//! DB-driven `SiteCatalog::default_llm_factory()` is used, as before this
//! parameter existed); a scripted-`Llm` integration test
//! (`tests/assistant_session_flow.rs`) passes `Some(..)` so a sub-agent spawn
//! test doesn't depend on a live model backend for a deterministic tool-call
//! decision.
//!
//! `SiteEngine::spawn` wires `tool_runner::spawn_tool_executor_with_policy`
//! and `persistence::spawn_persistence_subscriber_with_sink` at construction,
//! and sets `EngineConfig.idle_ttl` to 30 minutes as the "optional idle_ttl
//! backstop" the issue calls for — the next phase does not need to call
//! `Holly::hibernate` itself unless it wants tighter control.

mod profiles;
mod prompt_cache;
mod session_tree;

pub use profiles::{BUILD_PROFILE, PAGE_WRITER_PROFILE, RESEARCHER_PROFILE, SWITCHABLE_PROFILES};
use prompt_cache::load_system_prompt;
use session_tree::evict_on_hibernate_or_end;
pub use session_tree::{
    record_session_started, root_session_of, user_id_from_session, user_id_from_session_awaiting,
};

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::Context;
use dashmap::DashSet;
use entanglement_core::{
    AgentProfile, EngineConfig, Holly, OutEvent, Permission, PermissionProfile, SessionId,
    SystemPromptResolver, ToolSpec, ToolSpecResolver,
};
use entanglement_runtime::hooks::Hooks;
use entanglement_runtime::persistence::spawn_persistence_subscriber_with_sink;
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use entanglement_runtime::skills::SkillRegistry;
use entanglement_runtime::tool_runner;
use parking_lot::RwLock;
use sea_orm::DatabaseConnection;
use tokio::sync::broadcast;

use crate::ai::catalog::SiteCatalog;
use crate::ai::config::AiConfig;
use crate::ai::mcp::SiteMcp;
use crate::ai::persistence::{self, DbSink};
use crate::ai::policy::SitePolicy;
use crate::ai::tools;
use crate::routes::ws::WsHub;

/// A session-scoped tool spec cache: local specs are always present (baked
/// into the resolver closure at construction); this only ever holds the
/// *extra* per-session MCP specs layered on top (see `tool_spec_resolver`
/// below). The next phase's session handlers populate/clear this after
/// wiring/changing a session's enabled MCP servers.
type ToolSpecCache = Arc<RwLock<HashMap<SessionId, Vec<ToolSpec>>>>;

/// Auto-hibernate a settled, idle session after this long — the "optional
/// idle_ttl backstop" so a long-forgotten browser tab doesn't pin memory
/// forever. Not a hard cap on conversation length; only touches sessions
/// already at rest (see `EngineConfig.idle_ttl` docs).
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);

pub struct SiteEngine {
    pub holly: Holly,
    pub catalog: Arc<SiteCatalog>,
    pub policy: Arc<SitePolicy>,
    pub mcp: Arc<SiteMcp>,
    session_tool_specs: ToolSpecCache,
    system_prompt_cache: Arc<RwLock<String>>,
    /// Root sessions this process instance has confirmed have a live in-memory
    /// `Holly` task — either resumed via [`ensure_live`][Self::ensure_live] or
    /// freshly spawned by `create` (see [`mark_live`][Self::mark_live]).
    /// `Holly::resume` refuses an already-live id (see its doc), and sending
    /// any other `InMsg` to an id this process has never touched lazily spawns
    /// a **blank** session rather than replaying history — so callers must
    /// resume once, before the first send, and this set is what makes that
    /// "once" instead of "every message". Deliberately a flat per-process set,
    /// not a generalized cache — KISS, per the issue.
    ///
    /// `Arc`-wrapped (rather than the bare `DashSet` this started as) so the
    /// `hibernate_watcher` task below can hold its own clone: `Holly`'s own
    /// idle-TTL sweep (`EngineConfig.idle_ttl`, set below) evicts a settled
    /// session from *its* bookkeeping without this site ever calling
    /// `hibernate` itself, so nothing else would otherwise tell this cache to
    /// drop the id — see the watcher's doc.
    live_sessions: Arc<DashSet<SessionId>>,
    // Kept alive for the process lifetime (a tokio task keeps running once
    // spawned regardless of whether its `JoinHandle` is dropped, but holding
    // these documents intent and leaves room for a future graceful shutdown).
    #[allow(dead_code)]
    tool_executor: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    persistence_task: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    refresh_task: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    hibernate_watcher: tokio::task::JoinHandle<()>,
}

impl SiteEngine {
    /// Mint a fresh root session id for `user_id` — the sole place this site
    /// picks its tenant naming convention (`u{user_id}:{uuid}`).
    pub fn session_id_for_user(user_id: i32) -> SessionId {
        SessionId::new(format!("u{user_id}:{}", SessionId::new_uuid()))
    }

    /// Layer `specs` on top of the constant local tool specs for `session` —
    /// call after the next phase wires or changes a session's enabled MCP
    /// servers (e.g. from `crate::ai::mcp::SiteMcp::tool_specs_for_user`).
    pub fn set_session_mcp_specs(&self, session: SessionId, specs: Vec<ToolSpec>) {
        self.session_tool_specs.write().insert(session, specs);
    }

    /// Drop a session's per-session MCP specs (e.g. on session close).
    pub fn clear_session_mcp_specs(&self, session: &SessionId) {
        self.session_tool_specs.write().remove(session);
    }

    /// Build and spawn the engine: the local (built-in, non-MCP) tool
    /// registry wrapped once in a `SharedRegistry`, the model catalog, the
    /// permission policy, the tool executor, and the DB-backed persistence
    /// subscriber. `SiteMcp` (below) registers/deregisters a user's MCP tools
    /// into the same `SharedRegistry` live, on connect/change, so this never
    /// needs to seed or rebuild it. `llm_factory_override` is a test-only
    /// seam — see the module doc.
    pub async fn spawn(
        db: DatabaseConnection,
        ai_config: Arc<AiConfig>,
        ws_hub: Arc<WsHub>,
        serper_api_key: Option<String>,
        llm_factory_override: Option<entanglement_core::LlmFactory>,
    ) -> anyhow::Result<Arc<Self>> {
        let catalog = SiteCatalog::load(db.clone())
            .await
            .context("loading model catalog")?;
        let policy = SitePolicy::new(db.clone());

        let registry = tools::registry(Arc::new(db.clone()), ws_hub.clone(), serper_api_key);
        // The resolver's constant baseline (see `tool_spec_resolver` below):
        // it must never include another user's MCP tool identities, only the
        // per-session `SiteMcp::tool_specs_for_user` extra layered on top of
        // it does that (session-scoped, not process-global).
        let local_specs = registry.specs();
        let shared_registry = registry.shared();
        let mcp = SiteMcp::new(db.clone(), shared_registry.clone());

        let session_tool_specs: ToolSpecCache = Arc::new(RwLock::new(HashMap::new()));
        let initial_prompt = load_system_prompt(&db, &ai_config.system_prompt).await;
        let system_prompt_cache = Arc::new(RwLock::new(initial_prompt));

        let tool_spec_resolver: ToolSpecResolver = {
            let cache = session_tool_specs.clone();
            let local_specs = local_specs.clone();
            Arc::new(move |session: &SessionId| {
                let mut specs = local_specs.clone();
                if let Some(extra) = cache.read().get(session) {
                    specs.extend(extra.iter().cloned());
                }
                specs
            })
        };
        let system_prompt_resolver: SystemPromptResolver = {
            let cache = system_prompt_cache.clone();
            Arc::new(move |_session: &SessionId, profile: &AgentProfile| {
                let base = cache.read().clone();
                let suffix = match profile.name.as_str() {
                    RESEARCHER_PROFILE => profiles::RESEARCHER_PROMPT_SUFFIX,
                    PAGE_WRITER_PROFILE => profiles::PAGE_WRITER_PROMPT_SUFFIX,
                    _ => "",
                };
                Some(format!("{base}{suffix}"))
            })
        };

        // The root profile plus the two spawnable sub-agents (#17) —
        // `researcher`/`page-writer`; this site defines no plan/explore/debug
        // primaries.
        let profiles = profiles::build_profiles();
        // `agent_spawn`/`agent`/`agent_poll` are advertised per-profile, not
        // via the shared `tool_specs` (a non-spawning profile gets nothing) —
        // see `entanglement_runtime::subagent::spawn_specs_for`'s doc.
        let profile_tool_specs: HashMap<String, Vec<ToolSpec>> = profiles
            .iter()
            .filter_map(|p| {
                let specs = entanglement_runtime::subagent::spawn_specs_for(p, &profiles);
                (!specs.is_empty()).then(|| (p.name.clone(), specs))
            })
            .collect();
        let cfg = EngineConfig {
            // Defer to the catalog's *current* default at call time (not a
            // frozen snapshot) so an admin default-model change via
            // `catalog.refresh()` takes effect for un-pinned/resumed sessions
            // without a restart — otherwise the engine keeps calling whatever
            // model was default when the process booted.
            llm_factory: llm_factory_override.unwrap_or_else(|| catalog.dynamic_default_factory()),
            tool_specs: local_specs.clone(),
            profiles: profiles.clone(),
            profile_tool_specs,
            tool_spec_resolver: Some(tool_spec_resolver),
            system_prompt_resolver: Some(system_prompt_resolver),
            model_resolver: Some(catalog.model_resolver()),
            generation_resolver: Some(catalog.generation_resolver()),
            // The default model's own window, not `EngineConfig::default()`'s
            // generic fallback (#40) — a freshly-minted session's very first
            // turn (before any `SetModel` lands, e.g. a `/compact` fork's
            // seeded prompt, `handlers/sessions/compact.rs`) budgets against
            // this until `SetModel`/replay narrows it to the actual pinned
            // model via `ResolvedModel::context_window` (`catalog.rs`).
            context_window: catalog.default_model().and_then(|m| m.context_window),
            idle_ttl: Some(IDLE_TTL),
            // Explicit despite matching `EngineConfig::default()` (#40): on
            // overflow the turn loop LLM-summarizes the oldest history in
            // place (ADR-0103) instead of the lossy placeholder-prune this
            // site ran with before `context_window` above was ever populated.
            auto_compact: true,
            ..EngineConfig::default()
        };
        cfg.validate().map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let holly = Holly::spawn(cfg);

        let live_sessions: Arc<DashSet<SessionId>> = Arc::new(DashSet::new());
        // Tracks session liveness (`live_sessions`) and, for #17, sub-agent
        // parent links (`session_tree`) — both folded from the same
        // `Holly::subscribe()` broadcast `mcp.rs`/`ws_bridge.rs` also tap.
        let hibernate_watcher = {
            let live = live_sessions.clone();
            let mut sub = holly.subscribe();
            tokio::spawn(async move {
                loop {
                    match sub.recv().await {
                        Ok(ev) => {
                            evict_on_hibernate_or_end(&live, &ev);
                            if let OutEvent::SessionStarted {
                                session, parent, ..
                            } = &ev
                            {
                                record_session_started(session.clone(), parent.clone());
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        };

        let resolver: Arc<dyn PermissionResolver> = policy.clone();
        let grants: Arc<dyn GrantStore> = policy.clone();
        let tool_executor = tool_runner::spawn_tool_executor_with_policy(
            &holly,
            shared_registry,
            Arc::new(StdRwLock::new(profiles.clone())),
            Arc::new(StdRwLock::new(Arc::new(SkillRegistry::default()))),
            PermissionProfile::new(Permission::Allow),
            Arc::new(StdMutex::new(HashMap::new())),
            resolver,
            grants,
            Hooks::default(),
            None,
        );

        let sink: Arc<dyn entanglement_runtime::persistence::RecordSink> =
            Arc::new(DbSink::new(db.clone()));
        let persistence_task = spawn_persistence_subscriber_with_sink(&holly, sink);

        let refresh_task = prompt_cache::spawn_refresh_task(
            db.clone(),
            ai_config.system_prompt.clone(),
            system_prompt_cache.clone(),
        );

        Ok(Arc::new(SiteEngine {
            holly,
            catalog,
            policy,
            mcp,
            session_tool_specs,
            system_prompt_cache,
            live_sessions,
            tool_executor,
            persistence_task,
            refresh_task,
            hibernate_watcher,
        }))
    }

    /// Record `session` as already live in this process — call right after
    /// minting a brand-new session id and sending its first `InMsg` (which
    /// lazily spawns it blank, correctly, since there's no history yet).
    /// Skips a wasted (and refused) `resume` if a later handler call touches
    /// the same session in this process.
    pub fn mark_live(&self, session: SessionId) {
        self.live_sessions.insert(session);
    }

    /// Drop `session` from the live-tracking set — call once the engine has
    /// been told to close it (`InMsg::CloseSession`). Mostly hygiene (closed
    /// ids are single-use and never resurrected), so this just bounds the
    /// set's size over a long process lifetime rather than fixing a
    /// correctness issue.
    pub fn forget_live(&self, session: &SessionId) {
        self.live_sessions.remove(session);
    }

    /// Ensure `session` has a live in-memory task before sending it a
    /// `Prompt`/`Approve`/`SetModel` — resuming from `assistant_events` if
    /// this process hasn't touched it yet (a fresh process, or a session
    /// nobody has opened since it started). Idempotent and cheap once a
    /// session has been confirmed live (a single set lookup).
    pub async fn ensure_live(
        &self,
        db: &DatabaseConnection,
        session: SessionId,
    ) -> anyhow::Result<()> {
        if self.live_sessions.contains(&session) {
            return Ok(());
        }
        persistence::resume_session(db, &self.holly, session.clone())
            .await
            .with_context(|| format!("resuming engine session `{}`", session.0))?;
        self.live_sessions.insert(session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_round_trips_through_user_id_from_session() {
        let session = SiteEngine::session_id_for_user(7);
        assert!(session.0.starts_with("u7:"));
        assert_eq!(user_id_from_session(&session).unwrap(), 7);
    }

    #[test]
    fn distinct_users_mint_distinct_prefixes() {
        let a = SiteEngine::session_id_for_user(1);
        let b = SiteEngine::session_id_for_user(2);
        assert_ne!(
            user_id_from_session(&a).unwrap(),
            user_id_from_session(&b).unwrap()
        );
    }

    // `evict_on_hibernate_or_end`'s own behavior (live-cache eviction, parent-
    // cache forgetting, and that it ignores unrelated events) is tested in
    // `session_tree.rs`, right next to the function itself.
}
