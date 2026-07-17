//! DB-backed tests for `site::repo::tags::resolve_ids` against a real Postgres.
//! Gated on `DATABASE_URL` — skips with a message (not a failure) when unset, so
//! `cargo test`/`make verify` stays green without a live test DB.
//!
//! Each test creates its own throwaway `tags` row (unique name) and deletes it
//! when done. `site_test` isn't reset between runs, so this isolation matters.

use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::entity::tag;
use site::repo::tags::resolve_ids;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

async fn make_tag(db: &DatabaseConnection) -> (i32, String) {
    let name = format!("resolve-test-{}", uuid::Uuid::new_v4());
    let saved = tag::ActiveModel {
        name: Set(name.clone()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway tag");
    (saved.id, name)
}

#[tokio::test]
async fn unknown_names_are_skipped_not_errored() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let (known_id, known_name) = make_tag(&db).await;
    let missing_name = format!("definitely-missing-{}", uuid::Uuid::new_v4());

    let resolved = resolve_ids(&db, &[known_name, missing_name.clone()])
        .await
        .expect("resolve_ids must not error on unknown names");

    assert_eq!(resolved.ids, vec![known_id]);
    assert_eq!(resolved.missing, vec![missing_name]);

    tag::Entity::delete_by_id(known_id)
        .exec(&db)
        .await
        .expect("delete throwaway tag");
}

#[tokio::test]
async fn empty_input_resolves_to_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let resolved = resolve_ids(&db, &[]).await.expect("resolve_ids on empty");
    assert!(resolved.ids.is_empty());
    assert!(resolved.missing.is_empty());
}
