//! DB-backed coverage for `site::export::DbAssetProvider` (#65) — the
//! content-addressed `file_blobs` resolution path, plus `DbAssetProvider`
//! end to end against a real `DatabaseConnection` (the design-bundle/template
//! path alone is pure logic and also covered by the unit tests in
//! `src/export/assets.rs` per the repo's testing convention, see
//! `docs/testing.md`).
//!
//! Gated on `DATABASE_URL` — skips with a message (not a failure) when
//! unset, so `cargo test`/`make verify` stays green without a live test DB.
//! Each test creates its own throwaway `users`/`files`/`file_blobs` rows and
//! deletes them when done (`site_test` isn't reset between runs).

use mdcast::AssetProvider;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::design::DesignStore;
use site::entity::{file, file_blob, user};
use site::export::DbAssetProvider;
use std::sync::Arc;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

struct Fixture {
    user_id: i32,
    file_id: i32,
    hash: String,
    path: String,
}

async fn make_fixture(db: &DatabaseConnection, path: &str, content: &[u8]) -> Fixture {
    let username = format!("export-assets-{}", uuid::Uuid::new_v4());
    let saved_user = user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user");

    let hash = site::files::hash_blob(content);
    site::files::put_blob(db, &hash, content)
        .await
        .expect("put_blob");

    let saved_file = file::ActiveModel {
        hash: Set(hash.clone()),
        mimetype: Set("image/png".to_string()),
        path: Set(path.to_string()),
        description: Set(None),
        size_bytes: Set(content.len() as i64),
        created_by: Set(saved_user.id),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway file");

    Fixture {
        user_id: saved_user.id,
        file_id: saved_file.id,
        hash,
        path: path.to_string(),
    }
}

async fn cleanup(db: &DatabaseConnection, fx: &Fixture) {
    file::Entity::delete_by_id(fx.file_id)
        .exec(db)
        .await
        .expect("delete throwaway file");
    file_blob::Entity::delete_by_id(fx.hash.clone())
        .exec(db)
        .await
        .expect("delete throwaway file_blob");
    user::Entity::delete_by_id(fx.user_id)
        .exec(db)
        .await
        .expect("delete throwaway user");
}

#[tokio::test]
async fn resolves_a_page_content_image_from_file_blobs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let content = b"not really a png, just fixture bytes";
    let path = format!("export-assets-test/{}.png", uuid::Uuid::new_v4());
    let fx = make_fixture(&db, &path, content).await;

    let design = Arc::new(DesignStore::new(None));
    let provider = DbAssetProvider::new(db.clone(), design);

    let bytes = provider
        .get(&fx.path)
        .await
        .expect("get must not error")
        .expect("blob must resolve");
    assert_eq!(&bytes[..], content);

    cleanup(&db, &fx).await;
}

#[tokio::test]
async fn missing_content_path_resolves_to_none_not_an_error() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let design = Arc::new(DesignStore::new(None));
    let provider = DbAssetProvider::new(db, design);

    let missing = format!(
        "export-assets-test/does-not-exist-{}.png",
        uuid::Uuid::new_v4()
    );
    let result = provider.get(&missing).await.expect("get must not error");
    assert!(result.is_none());
}

#[tokio::test]
async fn a_content_path_colliding_with_a_template_prefix_still_falls_back_to_file_blobs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    // `reference/` is one of mdcast's own template-namespace prefixes
    // (mirrors `embedded/reference` in the mdcast crate); a page author is
    // free to upload real content at a colliding path, and `get` must not
    // let the (here, nonexistent) design-bundle lookup shadow it.
    let content = b"a handout uploaded under a colliding path";
    let path = format!("reference/{}.pdf", uuid::Uuid::new_v4());
    let fx = make_fixture(&db, &path, content).await;

    let design = Arc::new(DesignStore::new(None));
    let provider = DbAssetProvider::new(db.clone(), design);

    let bytes = provider
        .get(&fx.path)
        .await
        .expect("get must not error")
        .expect("colliding content path must still resolve via file_blobs");
    assert_eq!(&bytes[..], content);

    cleanup(&db, &fx).await;
}

#[tokio::test]
async fn resolves_a_template_from_the_design_bundle_and_lists_its_siblings() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "export_assets_template_test_{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(dir.join("mdcast/typst/layouts/pdf")).unwrap();
    std::fs::write(
        dir.join("mdcast/typst/layouts/pdf/default.typ"),
        b"#let brand = context.brand",
    )
    .unwrap();

    let design = Arc::new(DesignStore::new(Some(dir.clone())));
    let provider = DbAssetProvider::new(db, design);

    let bytes = provider
        .get("typst/layouts/pdf/default.typ")
        .await
        .expect("get must not error")
        .expect("template must resolve from the DESIGN_DIR override");
    assert_eq!(&bytes[..], b"#let brand = context.brand");

    let listed = provider
        .list("typst/layouts/pdf/")
        .await
        .expect("list must not error");
    assert_eq!(listed, vec!["typst/layouts/pdf/default.typ".to_string()]);

    // A page-content prefix has no directory-listing semantics.
    let content_listed = provider.list("images/").await.expect("list must not error");
    assert!(content_listed.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
