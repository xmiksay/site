//! End-to-end coverage for `markdown::render_for_export` (#66) — the
//! directive pre-render bridge that resolves `<fen>`/`<pgn>`/`<mermaid>`/
//! `<json>`/`<page>`/`<file>`/`<image>`/`<gallery>` to plain markdown (real
//! image refs, real pipe tables, recursively-spliced page markdown) instead
//! of the client-JS-dependent HTML `render()` produces for the browser.
//!
//! Gated on `DATABASE_URL` — skips with a message (not a failure) when
//! unset, same convention as `tests/export_assets.rs`. Every test creates its
//! own throwaway `users`/`files`/`file_blobs`/`galleries`/`pages` rows and
//! deletes them when done.

use std::sync::Arc;

use minijinja::Environment;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::design::DesignStore;
use site::entity::{file, file_blob, gallery, page, user};
use site::markdown::render_for_export;
use site::templates::Templates;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn tmpl_env() -> Arc<Environment<'static>> {
    Templates::new(Arc::new(DesignStore::new(None))).env()
}

async fn make_user(db: &DatabaseConnection) -> i32 {
    let username = format!("export-bridge-{}", uuid::Uuid::new_v4());
    user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user")
    .id
}

async fn delete_user(db: &DatabaseConnection, user_id: i32) {
    user::Entity::delete_by_id(user_id)
        .exec(db)
        .await
        .expect("delete throwaway user");
}

// ---------------------------------------------------------------------------
// <fen> / <pgn> — inline body, no DB rows needed beyond the connection itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inline_fen_directive_resolves_to_svg_asset() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "<fen>rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1</fen>";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<fen"),
        "got: {}",
        bridged.markdown
    );
    assert!(
        bridged.markdown.contains("![Chess position](bridge/fen/"),
        "got: {}",
        bridged.markdown
    );
    assert_eq!(bridged.assets.len(), 1);
    assert!(bridged.assets[0].1.starts_with(b"<svg"));
}

#[tokio::test]
async fn inline_pgn_directive_resolves_to_svg_asset() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "<pgn move=\"3\">1. e4 e5 2. Nf3 Nc6</pgn>";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<pgn"),
        "got: {}",
        bridged.markdown
    );
    assert_eq!(bridged.assets.len(), 1);
    assert!(bridged.assets[0].0.contains("bridge/pgn/"));
    assert!(bridged.assets[0].1.starts_with(b"<svg"));
}

#[tokio::test]
async fn invalid_inline_fen_reports_an_error_marker_without_panicking() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "<fen>this is not a fen at all</fen>";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(
        bridged.markdown.contains("*[fen:"),
        "got: {}",
        bridged.markdown
    );
    assert!(bridged.assets.is_empty());
}

// ---------------------------------------------------------------------------
// <gallery> — real DB-backed files.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gallery_directive_resolves_to_real_file_paths() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();
    let user_id = make_user(&db).await;

    let content = b"fixture image bytes";
    let hash = site::files::hash_blob(content);
    site::files::put_blob(&db, &hash, content)
        .await
        .expect("put_blob");

    let file_path = format!("export-bridge-test/{}.png", uuid::Uuid::new_v4());
    let saved_file = file::ActiveModel {
        hash: Set(hash.clone()),
        mimetype: Set("image/png".to_string()),
        path: Set(file_path.clone()),
        description: Set(None),
        size_bytes: Set(content.len() as i64),
        created_by: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway file");

    let gallery_path = format!("export-bridge-test/gallery-{}", uuid::Uuid::new_v4());
    let saved_gallery = gallery::ActiveModel {
        path: Set(gallery_path.clone()),
        title: Set("Bridge Test Gallery".to_string()),
        description: Set(None),
        file_ids: Set(vec![saved_file.id]),
        created_by: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway gallery");

    let md = format!(r#"<gallery path="{gallery_path}">"#);
    let bridged = render_for_export(&md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<gallery"),
        "got: {}",
        bridged.markdown
    );
    assert!(
        bridged.markdown.contains(&format!("]({file_path})")),
        "expected an image ref to `{file_path}`, got: {}",
        bridged.markdown
    );

    gallery::Entity::delete_by_id(saved_gallery.id)
        .exec(&db)
        .await
        .expect("delete throwaway gallery");
    file::Entity::delete_by_id(saved_file.id)
        .exec(&db)
        .await
        .expect("delete throwaway file");
    file_blob::Entity::delete_by_id(hash)
        .exec(&db)
        .await
        .expect("delete throwaway file_blob");
    delete_user(&db, user_id).await;
}

// ---------------------------------------------------------------------------
// <page> transclusion — real DB-backed pages, plus recursion-guard coverage.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn page_transclusion_inlines_nested_markdown() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();
    let user_id = make_user(&db).await;

    let now = chrono::Utc::now().fixed_offset();
    let nested_path = format!("export-bridge-test/nested-{}", uuid::Uuid::new_v4());
    let nested = page::ActiveModel {
        path: Set(nested_path.clone()),
        summary: Set(None),
        markdown: Set("Hello from the nested page.".to_string()),
        tag_ids: Set(vec![]),
        private: Set(false),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway nested page");

    let md = format!(r#"<page path="{nested_path}">"#);
    let bridged = render_for_export(&md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<page"),
        "got: {}",
        bridged.markdown
    );
    assert!(
        !bridged.markdown.contains("<div"),
        "got: {}",
        bridged.markdown
    );
    assert!(
        bridged.markdown.contains("Hello from the nested page."),
        "got: {}",
        bridged.markdown
    );

    page::Entity::delete_by_id(nested.id)
        .exec(&db)
        .await
        .expect("delete throwaway nested page");
    delete_user(&db, user_id).await;
}

#[tokio::test]
async fn cyclic_page_transclusion_produces_skip_message_without_hanging() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();
    let user_id = make_user(&db).await;

    let now = chrono::Utc::now().fixed_offset();
    let a_path = format!("export-bridge-test/a-{}", uuid::Uuid::new_v4());
    let b_path = format!("export-bridge-test/b-{}", uuid::Uuid::new_v4());

    let a = page::ActiveModel {
        path: Set(a_path.clone()),
        summary: Set(None),
        markdown: Set(format!(r#"<page path="{b_path}">"#)),
        tag_ids: Set(vec![]),
        private: Set(false),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway page a");

    let b = page::ActiveModel {
        path: Set(b_path.clone()),
        summary: Set(None),
        markdown: Set(format!(r#"<page path="{a_path}">"#)),
        tag_ids: Set(vec![]),
        private: Set(false),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway page b");

    // Drive the very markdown page `a` holds, exactly like transcluding it
    // from a third page would — this must terminate rather than hang.
    let md = format!(r#"<page path="{b_path}">"#);
    let bridged = render_for_export(&md, &db, &tmpl, false).await;

    assert!(
        bridged.markdown.contains("recursive transclusion") && bridged.markdown.contains("skipped"),
        "got: {}",
        bridged.markdown
    );

    page::Entity::delete_by_id(a.id)
        .exec(&db)
        .await
        .expect("delete throwaway page a");
    page::Entity::delete_by_id(b.id)
        .exec(&db)
        .await
        .expect("delete throwaway page b");
    delete_user(&db, user_id).await;
}

// ---------------------------------------------------------------------------
// <json> — inline body, no DB rows needed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inline_json_directive_renders_a_pipe_table() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = r#"<json query=".rows[]" type="table">{"rows":[{"a":1},{"a":2}]}</json>"#;
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<table"),
        "got: {}",
        bridged.markdown
    );
    assert!(bridged.markdown.contains('|'), "got: {}", bridged.markdown);
}

// ---------------------------------------------------------------------------
// <mermaid> — inline body, valid and invalid source.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inline_mermaid_directive_resolves_to_svg_asset() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "<mermaid>\npie\n\"A\" : 1\n\"B\" : 2\n</mermaid>";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(
        !bridged.markdown.contains("<mermaid"),
        "got: {}",
        bridged.markdown
    );
    assert_eq!(bridged.assets.len(), 1);
    assert!(bridged.assets[0].0.contains("bridge/mermaid/"));
    assert!(bridged.assets[0].1.starts_with(b"<svg"));
}

#[tokio::test]
async fn invalid_mermaid_source_falls_back_to_a_fenced_text_block() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "<mermaid>\nthis is not a valid mermaid diagram !!!\n</mermaid>";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert!(bridged.assets.is_empty());
    assert!(
        bridged.markdown.contains("```text"),
        "got: {}",
        bridged.markdown
    );
    assert!(
        bridged
            .markdown
            .contains("this is not a valid mermaid diagram !!!"),
        "got: {}",
        bridged.markdown
    );
}

// ---------------------------------------------------------------------------
// Plain markdown, no directives — must pass through untouched.
// ---------------------------------------------------------------------------

/// Syntect highlighting is an HTML-postprocessing step (`render()`'s parse +
/// highlight + link-rewrite pipeline) that `render_for_export` deliberately
/// never runs — mdcast/pandoc/typst do their own code rendering from plain
/// markdown. "Surviving" `render_for_export` here means passing through
/// unhighlighted/unmangled, which is the correct, intended behavior, not a
/// missing feature.
#[tokio::test]
async fn plain_markdown_with_no_directives_passes_through_untouched() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tmpl = tmpl_env();

    let md = "Some plain paragraph text.\n\n```rust\nfn main() {}\n```\n";
    let bridged = render_for_export(md, &db, &tmpl, false).await;

    assert_eq!(bridged.markdown, md);
    assert!(bridged.assets.is_empty());
}
