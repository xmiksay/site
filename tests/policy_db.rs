//! DB-backed tests for `site::ai::policy`/`site::ai::tool_permissions` against
//! a real Postgres (`tool_permissions` has an FK to `users`, so this can't be
//! faked in-memory). Gated on `DATABASE_URL` — skips with a message (not a
//! failure) when unset, so `cargo test`/`make verify` stays green without a
//! live test DB, and runs for real against `site_test` locally/in CI.
//!
//! Each test creates its own throwaway `users` row (unique username) and
//! deletes it when done — `tool_permissions`/`user_mcp_servers` both cascade
//! on delete, so that one delete cleans up every rule/server row the test
//! inserted. `site_test` isn't reset between runs, so this isolation is
//! load-bearing, not decorative.

use entanglement_core::{ApprovalScope, Permission, SessionId};
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::ai::engine::{self, SiteEngine};
use site::ai::policy::SitePolicy;
use site::ai::tool_permissions::Effect;
use site::entity::{tool_permission, user, user_mcp_server};

/// `None` (with a printed skip notice) when `DATABASE_URL` isn't set, per the
/// repo's DB-test convention — every test in this file starts with this.
async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

async fn make_user(db: &DatabaseConnection, tag: &str) -> i32 {
    let username = format!("policy-test-{tag}-{}", uuid::Uuid::new_v4());
    let saved = user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user");
    saved.id
}

async fn cleanup_user(db: &DatabaseConnection, user_id: i32) {
    user::Entity::delete_by_id(user_id)
        .exec(db)
        .await
        .expect("delete throwaway user"); // cascades to tool_permissions rows
}

async fn add_rule(
    db: &DatabaseConnection,
    user_id: i32,
    name: &str,
    effect: Effect,
    priority: i32,
) {
    tool_permission::ActiveModel {
        user_id: Set(user_id),
        name: Set(name.to_string()),
        effect: Set(effect.as_str().to_string()),
        priority: Set(priority),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert tool_permission rule");
}

#[tokio::test]
async fn resolve_picks_lowest_priority_regardless_of_insert_order() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "priority").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    // Higher-priority-number (runs later) deny inserted first, lower-number
    // (runs first) allow inserted second — insertion order must not matter,
    // only `priority ASC, id ASC`.
    add_rule(&db, user_id, "bash", Effect::Deny, 50).await;
    add_rule(&db, user_id, "bash", Effect::Allow, 10).await;

    let effect = policy.resolve(&session, "bash", "").await;
    assert_eq!(effect, Permission::Allow);

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn resolve_breaks_priority_ties_by_id_ascending() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "tiebreak").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    // Same priority: the first-inserted (lower id) row must win.
    add_rule(&db, user_id, "bash", Effect::Allow, 10).await;
    add_rule(&db, user_id, "bash", Effect::Deny, 10).await;

    let effect = policy.resolve(&session, "bash", "").await;
    assert_eq!(effect, Permission::Allow);

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn resolve_expands_a_bare_capability_rule_to_its_member_tools() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "capability").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    add_rule(&db, user_id, "read", Effect::Allow, 10).await;

    // Every `read` member tool (#39) is graded identically...
    assert_eq!(
        policy.resolve(&session, "read_page", "").await,
        Permission::Allow
    );
    assert_eq!(
        policy.resolve(&session, "search_pages", "").await,
        Permission::Allow
    );
    // ...but a `write` tool is untouched, falling through to the Ask default.
    assert_eq!(
        policy.resolve(&session, "edit_page", "").await,
        Permission::Ask
    );

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn resolve_honors_an_argument_scoped_rule_over_the_tool_call_input() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "scoped").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    add_rule(&db, user_id, "edit_page(obsidian/*)", Effect::Deny, 10).await;

    assert_eq!(
        policy
            .resolve(&session, "edit_page", r#"{"path":"obsidian/rust"}"#)
            .await,
        Permission::Deny
    );
    // Outside the scoped path, falls through to the Ask default.
    assert_eq!(
        policy
            .resolve(&session, "edit_page", r#"{"path":"projects/x"}"#)
            .await,
        Permission::Ask
    );

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn resolve_fans_a_bare_capability_rule_out_to_an_annotated_mcp_tool() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "mcp-capability").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    // #39/ADR-0117: a server's config-side capability hint fans a bare
    // capability rule out to its `"{server}__{tool}"` identity.
    user_mcp_server::ActiveModel {
        user_id: Set(user_id),
        name: Set("docs".to_string()),
        url: Set("https://example.invalid/mcp".to_string()),
        enabled: Set(true),
        forward_user_token: Set(false),
        headers: Set(serde_json::json!({})),
        capabilities: Set(serde_json::json!({"search": "read"})),
        created_at: Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway user_mcp_server");

    add_rule(&db, user_id, "read", Effect::Allow, 10).await;

    assert_eq!(
        policy.resolve(&session, "docs__search", "").await,
        Permission::Allow
    );
    // A different, unannotated tool on the same server is untouched.
    assert_eq!(
        policy.resolve(&session, "docs__unrelated", "").await,
        Permission::Ask
    );

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn resolve_defaults_to_ask_with_no_matching_rule() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "default").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    assert_eq!(
        policy.resolve(&session, "anything", "").await,
        Permission::Ask
    );

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn always_grant_persists_a_row_and_is_granted_reflects_it() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "grant").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    assert!(!policy.is_granted(&session, "create_tag", None));

    policy
        .record(&session, "create_tag", None, ApprovalScope::Always)
        .await;

    assert!(policy.is_granted(&session, "create_tag", None));

    let rows = tool_permission::Entity::find()
        .all(&db)
        .await
        .expect("query tool_permissions");
    let row = rows
        .iter()
        .find(|r| r.user_id == user_id && r.name == "create_tag")
        .expect("Always grant did not persist a tool_permissions row");
    assert_eq!(row.effect, "allow");

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn once_scope_is_not_persisted_as_a_grant() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "once").await;
    let policy = SitePolicy::new(db.clone());
    let session = SiteEngine::session_id_for_user(user_id);

    policy
        .record(&session, "delete_page", None, ApprovalScope::Once)
        .await;

    assert!(!policy.is_granted(&session, "delete_page", None));
    let rows = tool_permission::Entity::find()
        .all(&db)
        .await
        .expect("query tool_permissions");
    assert!(!rows.iter().any(|r| r.user_id == user_id));

    cleanup_user(&db, user_id).await;
}

/// #17: a `researcher`/`page-writer` sub-agent's child session is a bare,
/// unprefixed uuid (`entanglement_runtime::subagent::launch` mints it with no
/// tenant namespacing) — `SitePolicy::resolve` must still resolve it to the
/// *spawning user's* own `tool_permissions` rules, clamp intact, rather than
/// failing closed on the missing `u{user_id}:` prefix. This is the
/// clamp + resolver interplay the issue calls out: a user's `deny edit_page`
/// rule reaches into a `page-writer` child exactly as it would the root.
#[tokio::test]
async fn sub_agent_child_session_resolves_against_its_spawning_users_rules() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "subagent-clamp").await;
    let policy = SitePolicy::new(db.clone());
    let root = SiteEngine::session_id_for_user(user_id);
    let child = SessionId::new_uuid();

    // Normally fed by `engine.rs`'s session-lifecycle watcher off a live
    // `OutEvent::SessionStarted`; call the same recording path directly so
    // this test needs no running `Holly`/`agent_spawn` round trip.
    engine::record_session_started(child.clone(), Some(root.clone()));

    add_rule(&db, user_id, "edit_page", Effect::Deny, 10).await;

    // The clamp holds for both the child (bare uuid) and the root session —
    // same user, same rule, same effective grade.
    assert_eq!(
        policy.resolve(&child, "edit_page", "").await,
        Permission::Deny
    );
    assert_eq!(
        policy.resolve(&root, "edit_page", "").await,
        Permission::Deny
    );
    // A tool the rule doesn't cover still resolves normally through the
    // child, proving this isn't just an accidental blanket deny.
    assert_eq!(
        policy.resolve(&child, "read_page", "").await,
        Permission::Ask
    );

    cleanup_user(&db, user_id).await;
}
