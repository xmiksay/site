//! `SitePolicy` — the new engine's pluggable `PermissionResolver` +
//! `GrantStore` (`entanglement_runtime::policy`), backed by the existing
//! `tool_permissions` table / `crate::ai::tool_permissions::resolve` logic
//! instead of the CLI's file-backed defaults.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use entanglement_core::{ApprovalScope, Permission, SessionId};
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use parking_lot::RwLock;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

use crate::ai::engine::{user_id_from_session, user_id_from_session_awaiting};
use crate::ai::tool_permissions::{self, Effect};
use crate::entity::tool_permission;

/// Priority given to a grant recorded via `ApprovalScope::Always` — mirrors
/// today's approve-endpoint "remember" behavior (see
/// `src/ai/handlers/permissions.rs`'s equivalent insert, out of scope for
/// this phase but the same convention).
const REMEMBERED_GRANT_PRIORITY: i32 = 100;

pub struct SitePolicy {
    db: DatabaseConnection,
    /// Fast, sync snapshot of "always allow" grants recorded via `record()`,
    /// keyed by `(user_id, tool_name)` — `GrantStore::is_granted` must be
    /// synchronous (it's consulted inline before prompting), so it can't hit
    /// the DB directly. Kept in sync with the DB write inside `record()`;
    /// `invalidate_user` lets the next phase's permissions CRUD handlers
    /// force a re-sync after an out-of-band change (e.g. a rule deleted via
    /// the admin UI). `parking_lot::RwLock`, not `std::sync`: a panic while
    /// this is locked must not poison it and fail-closed every subsequent
    /// permission check for every user (issue #28).
    always_grants: RwLock<HashSet<(i32, String)>>,
}

impl SitePolicy {
    pub fn new(db: DatabaseConnection) -> Arc<Self> {
        Arc::new(SitePolicy {
            db,
            always_grants: RwLock::new(HashSet::new()),
        })
    }

    /// Drop every cached "always allow" grant for `user_id` — call after the
    /// next phase's tool-permissions CRUD handlers change or delete a rule
    /// out from under this cache. The next `PermissionResolver::resolve` call
    /// re-reads `tool_permissions` fresh regardless (only `is_granted`'s fast
    /// path is cached), so this only affects the `Ask`→`Allow` grant shortcut.
    pub fn invalidate_user(&self, user_id: i32) {
        self.always_grants
            .write()
            .retain(|(uid, _)| *uid != user_id);
    }
}

#[async_trait]
impl PermissionResolver for SitePolicy {
    async fn resolve(&self, session: &SessionId, tool: &str, _input: &str) -> Permission {
        // `_awaiting` (not the bare sync lookup): this runs in a detached
        // per-call dispatch task with no ordering guarantee relative to
        // `engine.rs`'s session watcher, which is what actually links a
        // freshly-spawned sub-agent child to its root — see that function's
        // doc. A plain `user_id_from_session` here would intermittently
        // fail-closed a legitimate sub-agent's very first tool call.
        let user_id = match user_id_from_session_awaiting(session).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, session = %session.0, "cannot resolve permission: bad session id");
                return Permission::Deny; // fail closed
            }
        };
        match tool_permissions::resolve(&self.db, user_id, tool).await {
            Ok(Effect::Allow) => Permission::Allow,
            Ok(Effect::Deny) => Permission::Deny,
            Ok(Effect::Prompt) => Permission::Ask,
            Err(e) => {
                tracing::error!(error = %e, user_id, tool, "tool_permissions lookup failed");
                Permission::Deny // fail closed
            }
        }
    }
}

#[async_trait]
impl GrantStore for SitePolicy {
    fn is_granted(&self, session: &SessionId, tool: &str, _arg: Option<&str>) -> bool {
        let Ok(user_id) = user_id_from_session(session) else {
            return false;
        };
        self.always_grants
            .read()
            .contains(&(user_id, tool.to_string()))
    }

    async fn record(
        &self,
        session: &SessionId,
        tool: &str,
        _arg: Option<&str>,
        scope: ApprovalScope,
    ) {
        if scope != ApprovalScope::Always {
            // `Once`/`Session` grants aren't persisted; entanglement-runtime
            // already re-asks on the next call for those scopes, matching
            // today's approve-endpoint behavior (only "remember" persists).
            return;
        }
        let Ok(user_id) = user_id_from_session(session) else {
            tracing::error!(session = %session.0, "cannot record grant: bad session id");
            return;
        };
        let row = tool_permission::ActiveModel {
            user_id: Set(user_id),
            name: Set(tool.to_string()),
            effect: Set(Effect::Allow.as_str().to_string()),
            priority: Set(REMEMBERED_GRANT_PRIORITY),
            ..Default::default()
        };
        if let Err(e) = row.insert(&self.db).await {
            tracing::error!(error = %e, user_id, tool, "failed to persist remembered tool grant");
            return;
        }
        self.always_grants
            .write()
            .insert((user_id, tool.to_string()));
    }

    fn forget_session(&self, _session: &SessionId) {
        // Grants here are user-scoped (the `tool_permissions` table), not
        // session-scoped, so a session ending has nothing to drop.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_from_session_round_trips_engine_session_id() {
        let session = crate::ai::engine::SiteEngine::session_id_for_user(42);
        assert_eq!(user_id_from_session(&session).unwrap(), 42);
    }

    #[test]
    fn user_id_from_session_rejects_malformed_ids() {
        assert!(user_id_from_session(&SessionId::new("no-prefix-here")).is_err());
        assert!(user_id_from_session(&SessionId::new("uNOTANUMBER:abc")).is_err());
        assert!(user_id_from_session(&SessionId::new("u:abc")).is_err());
    }

    /// The DB-backed half of "a sub-agent (`researcher`/`page-writer`) child
    /// session resolves permission against its *user's* `tool_permissions`
    /// rules, not fail closed on its unprefixed session id" is covered by
    /// `tests/policy_db.rs` (needs a real Postgres). This just confirms the
    /// piece `SitePolicy::resolve`/`is_granted`/`record` all share —
    /// `user_id_from_session` — resolves a sub-agent child the same as its
    /// root, which is `engine.rs`'s `SESSION_PARENTS` cache, exercised here
    /// via `crate::ai::engine`'s own test-only recording path.
    #[test]
    fn user_id_from_session_resolves_a_sub_agent_child_to_its_root_users_id() {
        use crate::ai::engine::root_session_of;
        use entanglement_core::SessionId as EngineSessionId;

        let root = crate::ai::engine::SiteEngine::session_id_for_user(99);
        let child = EngineSessionId::new_uuid();
        // `engine.rs`'s session-lifecycle watcher normally feeds this from a
        // live `OutEvent::SessionStarted`; call the same recording path it
        // uses so this test needs no running `Holly`.
        crate::ai::engine::record_session_started(child.clone(), Some(root.clone()));

        assert_eq!(root_session_of(&child), root);
        assert_eq!(user_id_from_session(&child).unwrap(), 99);
    }
}
