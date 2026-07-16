//! Sub-agent (#17) session ancestry: resolves the owning user for *any*
//! engine session, root or child, and the process-global cache that makes
//! that possible for a child whose id can't be parsed directly. Split out of
//! `engine.rs` to keep that file under the project's 400-line cap.
//!
//! ## Session id convention
//!
//! Every root session is minted `u{user_id}:{uuid}` (see
//! [`SiteEngine::session_id_for_user`][super::SiteEngine::session_id_for_user]);
//! every tool/policy call recovers the acting user via [`user_id_from_session`],
//! splitting on the first `:` (embedding.md §1's `{tenant}:{uuid}` convention,
//! with `u{user_id}` as this site's "tenant"). `policy.rs`, `tools/*`, and
//! `mcp.rs` all import this function (re-exported from `engine.rs`) rather
//! than re-deriving the format, so the convention has exactly one definition.
//!
//! A sub-agent (`agent_spawn`/`agent`) child session breaks the naked
//! convention: `entanglement_runtime` mints its id as a bare, unprefixed UUID.
//! [`user_id_from_session`] still resolves it correctly — it first walks the
//! child up to its root ancestor via [`root_session_of`] (backed by
//! [`SESSION_PARENTS`], fed by `engine.rs`'s `spawn`'s session-lifecycle
//! watcher) and only then parses the `u{user_id}:` prefix — so every existing
//! call site keeps working unchanged for sub-agent sessions too.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use dashmap::{DashMap, DashSet};
use entanglement_core::{OutEvent, SessionId};

/// Owning parent for every sub-agent session this process has seen started
/// (`OutEvent::SessionStarted { parent: Some(_), .. }`, fed by `engine.rs`'s
/// `spawn`'s session-lifecycle watcher). A root session needs no entry — its
/// engine `SessionId` already carries the `u{user_id}:` prefix
/// [`user_id_from_session`] parses directly.
///
/// Exists because `entanglement_runtime::subagent::launch` mints a child's id
/// as a bare `SessionId::new_uuid()` — upstream has no concept of this site's
/// per-tenant prefix, so a child's own id cannot be parsed the way a root's
/// can. Every `PermissionResolver`/tool call that only has a `SessionId` in
/// hand (this includes every call site across `policy.rs`, `tools/*.rs`, and
/// `mcp.rs` that already imports the free `user_id_from_session` function)
/// needs a way to still resolve the owning user for a child session — a
/// process-global cache here does that with **zero signature change** at any
/// of those call sites, instead of threading a shared cache through every
/// tool/policy/mcp constructor for a single-instance-per-process singleton.
///
/// Only ever grows (matches `SiteEngine::live_sessions`'s "flat, per-process,
/// KISS" tradeoff) except for the `SessionEnded`/`SessionHibernated` cleanup
/// [`forget`] does — a long-lived process accumulates one entry per sub-agent
/// ever spawned that hasn't settled, bounded in practice by
/// `entanglement_runtime`'s own per-root spawn budget (16).
static SESSION_PARENTS: OnceLock<DashMap<SessionId, SessionId>> = OnceLock::new();

fn session_parents() -> &'static DashMap<SessionId, SessionId> {
    SESSION_PARENTS.get_or_init(DashMap::new)
}

/// Record `session`'s parent link from its `SessionStarted` event. A root
/// (`parent: None`) needs nothing recorded (see [`SESSION_PARENTS`]'s doc).
/// `pub` (not `pub(crate)`) solely so `tests/policy_db.rs` — a separate crate
/// — can simulate a sub-agent child without driving a real `Holly` broadcast;
/// `engine.rs`'s `spawn`'s own session watcher is the only production caller.
pub fn record_session_started(session: SessionId, parent: Option<SessionId>) {
    if let Some(parent) = parent {
        session_parents().insert(session, parent);
    }
}

/// Drop `session`'s parent link — called from `engine.rs`'s
/// `evict_on_hibernate_or_end` on `SessionHibernated`/`SessionEnded`, so
/// [`SESSION_PARENTS`] doesn't outlive a settled sub-agent.
fn forget(session: &SessionId) {
    session_parents().remove(session);
}

/// Drop `ev`'s session from `live`/[`SESSION_PARENTS`] on
/// `SessionHibernated`/`SessionEnded` — the fix for the stale-liveness-cache
/// bug: `Holly`'s own idle-TTL sweep (or a manual `HibernateSession`) can evict
/// a session from *its* `sessions` map without this process ever being told to
/// forget it, so without this listener `SiteEngine::ensure_live` would keep
/// trusting a cache entry for a session `Holly` has already forgotten — the
/// next `InMsg` to it would then lazily respawn it **blank**, silently
/// discarding history that's still intact in `assistant_events`. Also bounds
/// [`SESSION_PARENTS`]'s growth: a settled sub-agent's parent link is no
/// longer needed once its session is gone. Factored out of `engine.rs`'s
/// session watcher task so it's testable without driving a real `Holly`
/// broadcast.
pub(super) fn evict_on_hibernate_or_end(live: &DashSet<SessionId>, ev: &OutEvent) {
    if let OutEvent::SessionHibernated { session, .. } | OutEvent::SessionEnded { session, .. } = ev
    {
        live.remove(session);
        forget(session);
    }
}

/// Walk [`SESSION_PARENTS`] up to `session`'s root ancestor (itself if it has
/// no recorded parent, i.e. it already is one). Cycle-guarded the same way
/// `entanglement_runtime::permission`'s own ancestor walks are, in case a
/// malformed/duplicated event ever links a session to itself transitively.
pub fn root_session_of(session: &SessionId) -> SessionId {
    let mut current = session.clone();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        match session_parents().get(&current).map(|p| p.clone()) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current
}

/// Recover the owning user id for `session` — a root or, via [`root_session_of`],
/// a sub-agent child of one. Fails closed (`Err`) on anything that doesn't
/// resolve to a `u{user_id}:{uuid}` root — callers (`policy.rs`, `tools/*`,
/// `mcp.rs`) all treat that as "deny"/"not found" rather than defaulting to
/// some user.
pub fn user_id_from_session(session: &SessionId) -> anyhow::Result<i32> {
    let root = root_session_of(session);
    let rest = root
        .0
        .strip_prefix('u')
        .with_context(|| format!("session id `{}` missing `u` tenant prefix", root.0))?;
    let (uid, _rest) = rest
        .split_once(':')
        .with_context(|| format!("session id `{}` missing `:` separator", root.0))?;
    uid.parse::<i32>()
        .with_context(|| format!("session id `{}` has a non-numeric user id", root.0))
}

/// How many times [`user_id_from_session_awaiting`] retries before giving up.
const SESSION_PARENT_RETRY_ATTEMPTS: u32 = 5;
/// Delay between each retry — [`SESSION_PARENTS`]'s writer (the session
/// watcher task) is a couple of `DashMap` operations on a lightweight event;
/// this only needs to outlast a scheduler hiccup, not real I/O.
const SESSION_PARENT_RETRY_DELAY: Duration = Duration::from_millis(5);

/// [`user_id_from_session`], but retried a few times on failure — closes the
/// TOCTOU window a bare `user_id_from_session` call has for a **freshly**
/// spawned sub-agent child: [`SESSION_PARENTS`] is populated by `engine.rs`'s
/// `spawn`'s session watcher task, itself an independent `holly.subscribe()`r
/// racing against whatever *other* task first needs to resolve that same
/// child's user (there is no cross-task ordering guarantee between two
/// independent broadcast subscribers, only within one). Unlike `ws_bridge.rs`
/// (which sidesteps the race entirely with its own locally-fed parent map,
/// since it already taps the same ordered stream), a detached per-call
/// dispatch task — `SitePolicy::resolve`'s caller — has no such stream to
/// piggyback on, so a short bounded retry is the practical fix: the
/// watcher's own work is microseconds, so in the near-certain case this
/// returns on the first try, and the worst case adds at most
/// `SESSION_PARENT_RETRY_ATTEMPTS * SESSION_PARENT_RETRY_DELAY` (25ms) to a
/// single permission resolution rather than silently denying it.
pub async fn user_id_from_session_awaiting(session: &SessionId) -> anyhow::Result<i32> {
    let mut last_err = None;
    for attempt in 0..SESSION_PARENT_RETRY_ATTEMPTS {
        match user_id_from_session(session) {
            Ok(uid) => return Ok(uid),
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < SESSION_PARENT_RETRY_ATTEMPTS {
                    tokio::time::sleep(SESSION_PARENT_RETRY_DELAY).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("unreachable: retry loop ran zero times")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::engine::SiteEngine;

    #[test]
    fn child_session_resolves_to_its_root_users_id() {
        let root = SiteEngine::session_id_for_user(4242);
        let child = SessionId::new_uuid();
        record_session_started(child.clone(), Some(root.clone()));

        assert_eq!(root_session_of(&child), root);
        assert_eq!(user_id_from_session(&child).unwrap(), 4242);
    }

    #[test]
    fn grandchild_session_resolves_through_the_full_ancestor_chain() {
        let root = SiteEngine::session_id_for_user(4343);
        let child = SessionId::new_uuid();
        let grandchild = SessionId::new_uuid();
        record_session_started(child.clone(), Some(root.clone()));
        record_session_started(grandchild.clone(), Some(child.clone()));

        assert_eq!(root_session_of(&grandchild), root);
        assert_eq!(user_id_from_session(&grandchild).unwrap(), 4343);
    }

    #[test]
    fn root_session_without_a_parent_link_resolves_to_itself() {
        let root = SiteEngine::session_id_for_user(4444);
        assert_eq!(root_session_of(&root), root);
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

    #[test]
    fn ended_child_session_is_forgotten_by_the_parent_cache() {
        let root = SiteEngine::session_id_for_user(4545);
        let child = SessionId::new_uuid();
        record_session_started(child.clone(), Some(root.clone()));
        assert_eq!(root_session_of(&child), root);

        let live: DashSet<SessionId> = DashSet::new();
        evict_on_hibernate_or_end(
            &live,
            &OutEvent::SessionEnded {
                session: child.clone(),
                ts: 0,
            },
        );

        // The link is gone, so the child (an unprefixed uuid) no longer
        // resolves to anyone — falls through to itself and fails to parse.
        assert_eq!(root_session_of(&child), child);
        assert!(user_id_from_session(&child).is_err());
    }

    #[tokio::test]
    async fn user_id_from_session_awaiting_succeeds_once_a_late_link_lands() {
        let root = SiteEngine::session_id_for_user(4646);
        let child = SessionId::new_uuid();
        // Not yet linked — a bare lookup would fail closed right now.
        assert!(user_id_from_session(&child).is_err());

        // Simulate the session watcher task recording the link a beat later
        // (the exact race this function exists to tolerate).
        let recorded_child = child.clone();
        let recorded_root = root.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            record_session_started(recorded_child, Some(recorded_root));
        });

        assert_eq!(user_id_from_session_awaiting(&child).await.unwrap(), 4646);
    }

    #[tokio::test]
    async fn user_id_from_session_awaiting_still_fails_closed_if_never_linked() {
        let child = SessionId::new_uuid();
        assert!(user_id_from_session_awaiting(&child).await.is_err());
    }
}
