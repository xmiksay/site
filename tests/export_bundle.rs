//! DB-backed coverage for `site::export::build_bundle` (mdcast 0.4 thin
//! client) — the digest-only declaration path against real `files`/
//! `file_blobs` rows (the pure pieces — image-key extraction, design-template
//! key mapping — are unit tests in `src/export/bundle.rs` per the repo's
//! testing convention, see `docs/testing.md`).
//!
//! Gated on `DATABASE_URL` — skips with a message (not a failure) when
//! unset, so `cargo test`/`make verify` stays green without a live test DB.
//! Each test creates its own throwaway `users`/`files`/`file_blobs` rows and
//! deletes them when done (`site_test` isn't reset between runs).

use bytes::Bytes;
use mdcast_api::BrandSpec;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::design::DesignStore;
use site::entity::{file, file_blob, user};
use site::export::{BridgedMarkdown, build_bundle};

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
    let username = format!("export-bundle-{}", uuid::Uuid::new_v4());
    let saved_user = user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user");

    let hash = site::files::hash_blob(content);
    site::storage::Storage::db(db.clone())
        .put_blob(content)
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

fn bridged(markdown: String, assets: Vec<(String, Bytes)>) -> BridgedMarkdown {
    BridgedMarkdown { markdown, assets }
}

#[tokio::test]
async fn declares_a_page_image_digest_only_with_the_files_hash() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let content = b"not really a png, just fixture bytes";
    let path = format!("export-bundle-test/{}.png", uuid::Uuid::new_v4());
    let fx = make_fixture(&db, &path, content).await;

    let design = DesignStore::new(None);
    let bundle = build_bundle(
        &db,
        &site::storage::Storage::db(db.clone()),
        &design,
        &BrandSpec::default(),
        &bridged(format!("![img]({})", fx.path), Vec::new()),
    )
    .await
    .expect("build_bundle must not error");

    let digest = bundle
        .manifest()
        .get(&fx.path)
        .cloned()
        .expect("the referenced image must be declared in the manifest");
    assert_eq!(
        digest.as_str(),
        fx.hash,
        "manifest digest must be files.hash"
    );
    assert!(
        bundle.get(&fx.path).is_none(),
        "a DB image is digest-only: bytes stay lazy until the server asks"
    );

    cleanup(&db, &fx).await;
}

#[tokio::test]
async fn missing_content_path_is_skipped_not_an_error() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let missing = format!(
        "export-bundle-test/does-not-exist-{}.png",
        uuid::Uuid::new_v4()
    );

    let design = DesignStore::new(None);
    let bundle = build_bundle(
        &db,
        &site::storage::Storage::db(db.clone()),
        &design,
        &BrandSpec::default(),
        &bridged(format!("![img]({missing})"), Vec::new()),
    )
    .await
    .expect("an unresolvable image ref must not fail the bundle");

    assert!(
        bundle.manifest().get(&missing).is_none(),
        "an image without a files row stays undeclared (server warn+drops it)"
    );
}

#[tokio::test]
async fn bridge_assets_are_eager_and_design_templates_shadow_the_catalog() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "export_bundle_template_test_{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(dir.join("mdcast/typst/layouts/pdf")).unwrap();
    std::fs::write(
        dir.join("mdcast/typst/layouts/pdf/default.typ"),
        b"#let brand = context.brand",
    )
    .unwrap();
    std::fs::write(dir.join("mdcast/brand.toml"), b"name = \"fixture\"").unwrap();

    let design = DesignStore::new(Some(dir.clone()));
    let svg_key = "bridge/fen/abc.svg".to_string();
    let bundle = build_bundle(
        &db,
        &site::storage::Storage::db(db.clone()),
        &design,
        &BrandSpec::default(),
        &bridged(
            format!("![Chess position]({svg_key})"),
            vec![(svg_key.clone(), Bytes::from_static(b"<svg/>"))],
        ),
    )
    .await
    .expect("build_bundle must not error");

    assert_eq!(
        bundle.get(&svg_key),
        Some(&Bytes::from_static(b"<svg/>")),
        "synthesized bridge SVGs are eager — they have no files row to fetch from"
    );
    assert!(
        bundle
            .manifest()
            .get("typst/layouts/pdf/default.typ")
            .is_some(),
        "design templates are declared under mdcast's own key space so they \
         shadow the server's embedded catalog"
    );
    assert!(
        bundle.manifest().get("brand.toml").is_none()
            && bundle.manifest().get("mdcast/brand.toml").is_none(),
        "brand.toml travels as request.brand, never as an asset"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
