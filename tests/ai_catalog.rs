//! DB-dependent tests for `site::ai::catalog::SiteCatalog` against real
//! `llm_providers`/`llm_models` rows. Gated on `DATABASE_URL`; follows
//! `tests/policy_db.rs`'s skip/cleanup convention. Neither table FKs to
//! `users` (`m_014_create_llm_providers.rs` / `m_015_split_llm_models.rs`), so
//! cleanup is a direct `llm_providers` delete — cascades to its `llm_models`
//! rows (`ON DELETE CASCADE` on `llm_models.provider_id`).
//!
//! Every provider row here uses `kind = "ollama"` with no `api_key` needed —
//! `build_factory`'s own per-`kind` dispatch (including the `api_key`-required
//! branches) is covered by the pure in-module tests at the bottom of
//! `src/ai/catalog.rs`; these tests are about the DB-hydrated catalog surface
//! (`default_model`, `model_by_id`, `model_resolver`), not that dispatch.
//!
//! Unlike `tests/policy_db.rs` (scoped by a unique `user_id` per test),
//! `SiteCatalog::load`/`refresh` reads the *entire* `llm_providers`/
//! `llm_models` tables with no per-test scoping — mirroring production, where
//! there's one site-wide catalog. `cargo test` runs test functions within a
//! binary concurrently by default, so two of these tests running at once
//! would race on which rows are visible when the other's `load` runs
//! (concretely: whichever row is physically first in an unordered table scan
//! decides the "no row flagged default" fallback). Serialize every test in
//! this file with one process-wide lock instead of relying on data isolation
//! that the code under test doesn't provide.

use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::ai::catalog::SiteCatalog;
use site::entity::{llm_model, llm_provider};
use tokio::sync::{Mutex, MutexGuard};

// `tokio::sync::Mutex` (not `std::sync::Mutex`): each test's body holds the
// guard across several `.await` points, which `std::sync::MutexGuard` isn't
// designed for (and clippy's `await_holding_lock` correctly flags).
static SERIALIZE: Mutex<()> = Mutex::const_new(());

/// Acquire the file-wide serialization lock (see the module doc for why every
/// test in this file needs one).
async fn exclusive() -> MutexGuard<'static, ()> {
    SERIALIZE.lock().await
}

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

async fn make_provider(db: &DatabaseConnection, tag: &str) -> llm_provider::Model {
    llm_provider::ActiveModel {
        label: Set(format!("catalog-test-{tag}-{}", uuid::Uuid::new_v4())),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway llm_provider")
}

async fn make_model(
    db: &DatabaseConnection,
    provider_id: i32,
    wire: &str,
    is_default: bool,
) -> llm_model::Model {
    llm_model::ActiveModel {
        provider_id: Set(provider_id),
        label: Set(format!("label-{wire}")),
        model: Set(wire.to_string()),
        is_default: Set(is_default),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway llm_model")
}

async fn cleanup_provider(db: &DatabaseConnection, provider_id: i32) {
    llm_provider::Entity::delete_by_id(provider_id)
        .exec(db)
        .await
        .expect("delete throwaway llm_provider"); // cascades to llm_models rows
}

/// Wipe every `llm_providers`/`llm_models` row, called right after acquiring
/// `exclusive()` and before inserting this test's own fixture. Guards against
/// more than just concurrent tests: if an *earlier* run of this file panicked
/// after inserting rows but before its own `cleanup_provider` ran, those rows
/// would otherwise sit in `site_test` (which isn't reset between runs) and
/// silently decide `default_model()`'s unordered-scan-dependent fallback for
/// every run after. Safe to wipe unconditionally — this file exclusively owns
/// these two tables (see the module doc); no other test file's fixtures live
/// here.
async fn wipe_catalog_tables(db: &DatabaseConnection) {
    llm_provider::Entity::delete_many()
        .exec(db)
        .await
        .expect("wipe llm_providers"); // cascades to llm_models
}

#[tokio::test]
async fn default_model_picks_the_row_flagged_default() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "flagged").await;
    make_model(&db, provider.id, "model-a", false).await;
    let flagged = make_model(&db, provider.id, "model-b", true).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    let default = catalog
        .default_model()
        .expect("expected a default model to resolve");
    assert_eq!(default.model_id, flagged.id);

    cleanup_provider(&db, provider.id).await;
}

#[tokio::test]
async fn default_model_falls_back_to_first_row_when_none_flagged() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "unflagged").await;
    let first = make_model(&db, provider.id, "model-a", false).await;
    make_model(&db, provider.id, "model-b", false).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    let default = catalog
        .default_model()
        .expect("expected a fallback default model to resolve");
    assert_eq!(default.model_id, first.id);

    cleanup_provider(&db, provider.id).await;
}

#[tokio::test]
async fn model_by_id_finds_and_misses() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "lookup").await;
    let model = make_model(&db, provider.id, "model-a", true).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    assert!(catalog.model_by_id(model.id).is_some());
    assert!(catalog.model_by_id(-1).is_none());

    cleanup_provider(&db, provider.id).await;
}

#[tokio::test]
async fn model_resolver_resolves_a_correct_provider_model_pair() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "resolve-ok").await;
    let model = make_model(&db, provider.id, "model-a", true).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    let resolver = catalog.model_resolver();
    let resolved = resolver(&provider.label, &model.id.to_string())
        .expect("expected the matching provider/model pair to resolve");
    assert_eq!(resolved.provider, provider.label);
    assert_eq!(resolved.model, "model-a");

    cleanup_provider(&db, provider.id).await;
}

#[tokio::test]
async fn model_resolver_rejects_a_model_id_under_the_wrong_provider_label() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "resolve-wrong-provider").await;
    let model = make_model(&db, provider.id, "model-a", true).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    let resolver = catalog.model_resolver();
    let err = resolver("not-the-real-label", &model.id.to_string())
        .err()
        .expect("a real model id under the wrong provider label must be rejected");
    assert!(err.contains("not `not-the-real-label`") || err.contains("belongs to provider"));

    cleanup_provider(&db, provider.id).await;
}

#[tokio::test]
async fn model_resolver_rejects_a_non_numeric_model_string() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = exclusive().await;
    wipe_catalog_tables(&db).await;
    let provider = make_provider(&db, "resolve-non-numeric").await;
    make_model(&db, provider.id, "model-a", true).await;

    let catalog = SiteCatalog::load(db.clone()).await.expect("load catalog");
    let resolver = catalog.model_resolver();
    let err = resolver(&provider.label, "not-a-number")
        .err()
        .expect("a non-numeric model string must be rejected");
    assert!(err.contains("not a valid model id"));

    cleanup_provider(&db, provider.id).await;
}
