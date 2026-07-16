//! `SiteEngine` — the single process-wide `Holly` (embedding.md §1: one
//! engine for every tenant, sessions namespaced per user) wired into
//! `state.rs` in place of the old `loop_driver`/`mcp_client` turn-driving
//! stack.
//!
//! ## Session id convention
//!
//! Every root session is minted `u{user_id}:{uuid}` (see
//! [`session_id_for_user`]); every tool/policy call recovers the acting user
//! via [`user_id_from_session`], splitting on the first `:` (embedding.md §1's
//! `{tenant}:{uuid}` convention, with `u{user_id}` as this site's "tenant").
//! `policy.rs`, `tools/*`, and `mcp.rs` all import these two functions rather
//! than re-deriving the format, so the convention has exactly one definition.
//!
//! ## Public API for the next phase
//!
//! ```ignore
//! let engine = SiteEngine::spawn(db, ai_config, serper_api_key).await?;
//! let session = SiteEngine::session_id_for_user(user_id);
//! engine.holly.send(InMsg::prompt(session, text)).await?;
//! ```
//!
//! `SiteEngine::spawn` wires `tool_runner::spawn_tool_executor_with_policy`
//! and `persistence::spawn_persistence_subscriber_with_sink` at construction,
//! and sets `EngineConfig.idle_ttl` to 30 minutes as the "optional idle_ttl
//! backstop" the issue calls for — the next phase does not need to call
//! `Holly::hibernate` itself unless it wants tighter control.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Context;
use dashmap::DashSet;
use entanglement_core::{
    EngineConfig, Holly, OutEvent, Permission, PermissionProfile, ProfileRegistry, SessionId,
    SystemPromptResolver, ToolSpec, ToolSpecResolver,
};
use entanglement_runtime::hooks::Hooks;
use entanglement_runtime::persistence::spawn_persistence_subscriber_with_sink;
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use entanglement_runtime::tool_runner;
use sea_orm::DatabaseConnection;
use tokio::sync::broadcast;

use crate::ai::catalog::SiteCatalog;
use crate::ai::config::{AiConfig, SYSTEM_PROMPT_PAGE_PATH};
use crate::ai::mcp::{McpRoutedTool, SiteMcp};
use crate::ai::persistence::{self, DbSink};
use crate::ai::policy::SitePolicy;
use crate::ai::tools;

/// A session-scoped tool spec cache: local specs are always present (baked
/// into the resolver closure at construction); this only ever holds the
/// *extra* per-session MCP specs layered on top (see `tool_spec_resolver`
/// below). The next phase's session handlers populate/clear this after
/// wiring/changing a session's enabled MCP servers.
type ToolSpecCache = Arc<RwLock<HashMap<SessionId, Vec<ToolSpec>>>>;

/// How often the background task re-reads the `system/prompt` page. The
/// resolver itself must stay a sync `Fn` (embedding.md §4's snapshot-cache
/// pattern) — this is how often the cache it reads gets refreshed, not how
/// often the DB is hit per-turn.
const SYSTEM_PROMPT_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

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
        self.session_tool_specs
            .write()
            .unwrap()
            .insert(session, specs);
    }

    /// Drop a session's per-session MCP specs (e.g. on session close).
    pub fn clear_session_mcp_specs(&self, session: &SessionId) {
        self.session_tool_specs.write().unwrap().remove(session);
    }

    /// Current effective system prompt (the DB `system/prompt` page's
    /// markdown if non-blank, else `AiConfig`'s fallback) — refreshed every
    /// `SYSTEM_PROMPT_REFRESH_INTERVAL` in the background. Exposed for e.g.
    /// an admin "preview effective prompt" surface in the next phase.
    pub fn system_prompt(&self) -> String {
        self.system_prompt_cache.read().unwrap().clone()
    }

    /// Build and spawn the engine: the local tool registry (+ any MCP tool
    /// identities already known across every user, see `mcp.rs`'s
    /// static-registry limitation doc), the model catalog, the permission
    /// policy, the tool executor, and the DB-backed persistence subscriber.
    pub async fn spawn(
        db: DatabaseConnection,
        ai_config: Arc<AiConfig>,
        serper_api_key: Option<String>,
    ) -> anyhow::Result<Arc<Self>> {
        let catalog = SiteCatalog::load(db.clone())
            .await
            .context("loading model catalog")?;
        let policy = SitePolicy::new(db.clone());
        let mcp = SiteMcp::new(db.clone());

        let db_arc = Arc::new(db.clone());
        let mut registry = tools::registry(db_arc, serper_api_key);
        let local_specs = registry.specs();
        for name in mcp.known_tool_names(&db).await {
            registry.register(McpRoutedTool::new(name, mcp.clone()));
        }

        let session_tool_specs: ToolSpecCache = Arc::new(RwLock::new(HashMap::new()));
        let initial_prompt = load_system_prompt(&db, &ai_config.system_prompt).await;
        let system_prompt_cache = Arc::new(RwLock::new(initial_prompt));

        let tool_spec_resolver: ToolSpecResolver = {
            let cache = session_tool_specs.clone();
            Arc::new(move |session: &SessionId| {
                let mut specs = local_specs.clone();
                if let Some(extra) = cache.read().unwrap().get(session) {
                    specs.extend(extra.iter().cloned());
                }
                specs
            })
        };
        let system_prompt_resolver: SystemPromptResolver = {
            let cache = system_prompt_cache.clone();
            Arc::new(
                move |_session: &SessionId, _profile: &entanglement_core::AgentProfile| {
                    Some(cache.read().unwrap().clone())
                },
            )
        };

        let profiles = ProfileRegistry::new(); // just the built-in `build` profile — this site has no plan/explore/debug sub-agents
        let cfg = EngineConfig {
            llm_factory: catalog.default_llm_factory(),
            tool_specs: registry.specs(),
            profiles: profiles.clone(),
            tool_spec_resolver: Some(tool_spec_resolver),
            system_prompt_resolver: Some(system_prompt_resolver),
            model_resolver: Some(catalog.model_resolver()),
            idle_ttl: Some(IDLE_TTL),
            ..EngineConfig::default()
        };
        cfg.validate().map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let holly = Holly::spawn(cfg);

        let live_sessions: Arc<DashSet<SessionId>> = Arc::new(DashSet::new());
        let hibernate_watcher = {
            let live = live_sessions.clone();
            let mut sub = holly.subscribe();
            tokio::spawn(async move {
                loop {
                    match sub.recv().await {
                        Ok(ev) => evict_on_hibernate_or_end(&live, &ev),
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
            registry,
            Arc::new(RwLock::new(profiles)),
            PermissionProfile::new(Permission::Allow),
            Arc::new(Mutex::new(HashMap::new())),
            resolver,
            grants,
            Hooks::default(),
        );

        let sink: Arc<dyn entanglement_runtime::persistence::RecordSink> =
            Arc::new(DbSink::new(db.clone()));
        let persistence_task = spawn_persistence_subscriber_with_sink(&holly, sink);

        let refresh_task = {
            let refresh_db = db.clone();
            let refresh_cache = system_prompt_cache.clone();
            let fallback = ai_config.system_prompt.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(SYSTEM_PROMPT_REFRESH_INTERVAL);
                interval.tick().await; // first tick fires immediately; skip the redundant reload
                loop {
                    interval.tick().await;
                    let prompt = load_system_prompt(&refresh_db, &fallback).await;
                    *refresh_cache.write().unwrap() = prompt;
                }
            })
        };

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

/// Split the `u{user_id}:{uuid}` convention back apart. Fails closed (`Err`)
/// on anything that doesn't match — callers (`policy.rs`, `tools/*`, `mcp.rs`)
/// all treat that as "deny"/"not found" rather than defaulting to some user.
pub fn user_id_from_session(session: &SessionId) -> anyhow::Result<i32> {
    let rest = session
        .0
        .strip_prefix('u')
        .with_context(|| format!("session id `{}` missing `u` tenant prefix", session.0))?;
    let (uid, _rest) = rest
        .split_once(':')
        .with_context(|| format!("session id `{}` missing `:` separator", session.0))?;
    uid.parse::<i32>()
        .with_context(|| format!("session id `{}` has a non-numeric user id", session.0))
}

/// Drop `ev`'s session from `live` on `SessionHibernated`/`SessionEnded` — the
/// fix for the stale-liveness-cache bug: `Holly`'s own idle-TTL sweep (or a
/// manual `HibernateSession`) can evict a session from *its* `sessions` map
/// without this process ever being told to forget it, so without this
/// listener `ensure_live` would keep trusting a cache entry for a session
/// `Holly` has already forgotten — the next `InMsg` to it would then lazily
/// respawn it **blank**, silently discarding history that's still intact in
/// `assistant_events`. Factored out of the `hibernate_watcher` task in `spawn`
/// so it's testable without driving a real `Holly` broadcast.
fn evict_on_hibernate_or_end(live: &DashSet<SessionId>, ev: &OutEvent) {
    if let OutEvent::SessionHibernated { session, .. } | OutEvent::SessionEnded { session, .. } = ev
    {
        live.remove(session);
    }
}

async fn load_system_prompt(db: &DatabaseConnection, fallback: &str) -> String {
    match crate::repo::pages::find_by_path(db, SYSTEM_PROMPT_PAGE_PATH).await {
        Ok(Some(page)) if !page.markdown.trim().is_empty() => page.markdown,
        Ok(_) => fallback.to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to load system/prompt page; using fallback");
            fallback.to_string()
        }
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

    #[test]
    fn hibernated_session_is_evicted_from_the_live_cache() {
        let live: DashSet<SessionId> = DashSet::new();
        let session = SiteEngine::session_id_for_user(9);
        live.insert(session.clone());

        evict_on_hibernate_or_end(
            &live,
            &OutEvent::SessionHibernated {
                session: session.clone(),
                ts: 0,
            },
        );

        assert!(!live.contains(&session));
    }

    #[test]
    fn session_ended_is_also_evicted_from_the_live_cache() {
        let live: DashSet<SessionId> = DashSet::new();
        let session = SiteEngine::session_id_for_user(10);
        live.insert(session.clone());

        evict_on_hibernate_or_end(
            &live,
            &OutEvent::SessionEnded {
                session: session.clone(),
                ts: 0,
            },
        );

        assert!(!live.contains(&session));
    }

    #[test]
    fn unrelated_event_does_not_evict_other_sessions() {
        let live: DashSet<SessionId> = DashSet::new();
        let session = SiteEngine::session_id_for_user(11);
        live.insert(session.clone());

        evict_on_hibernate_or_end(
            &live,
            &OutEvent::Done {
                session: session.clone(),
                seq: 1,
            },
        );

        assert!(live.contains(&session));
    }
}
