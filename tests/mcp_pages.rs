//! `tools/call` round trips for the `pages` tool family (`edit_page`,
//! `read_page`, `search_pages`, `delete_page` — `src/routes/mcp/pages.rs`)
//! through the real `POST /mcp` handler. Envelope-level scenarios (auth,
//! `initialize`, dispatch errors) live in the sibling `tests/mcp_endpoint.rs`;
//! shared fixture/helpers in `tests/common/mcp.rs`. Same DB-gated +
//! throwaway-user convention as `tests/policy_db.rs`.

#[path = "common/mcp.rs"]
mod mcp_common;

use mcp_common::{call_tool, cleanup_user, is_tool_error, setup, test_db_url, tool_text};
use serde_json::json;

#[tokio::test]
async fn edit_page_creates_then_read_page_returns_it_then_delete_page_removes_it() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "pages-roundtrip").await;
    let path = format!("mcp-test/{}", uuid::Uuid::new_v4());

    let created = call_tool(
        &fx.app,
        &fx.token,
        "edit_page",
        json!({
            "path": path,
            "markdown": "hello from the mcp test",
            "summary": "a throwaway test page",
        }),
    )
    .await;
    assert!(!is_tool_error(&created), "edit_page failed: {created:?}");
    assert!(tool_text(&created).starts_with("created: "));

    let read = call_tool(&fx.app, &fx.token, "read_page", json!({ "path": path })).await;
    assert!(!is_tool_error(&read), "read_page failed: {read:?}");
    let read_text = tool_text(&read);
    assert!(read_text.contains(&path));
    assert!(read_text.contains("hello from the mcp test"));
    assert!(read_text.contains("Summary: a throwaway test page"));

    let deleted = call_tool(&fx.app, &fx.token, "delete_page", json!({ "path": path })).await;
    assert!(!is_tool_error(&deleted), "delete_page failed: {deleted:?}");
    assert_eq!(tool_text(&deleted), format!("deleted: {path}"));

    let read_after_delete =
        call_tool(&fx.app, &fx.token, "read_page", json!({ "path": path })).await;
    assert!(is_tool_error(&read_after_delete));
    assert!(tool_text(&read_after_delete).contains("Page not found"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn search_pages_finds_a_page_by_path_prefix() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "pages-search").await;
    let prefix = format!("mcp-search-{}", uuid::Uuid::new_v4());
    let path = format!("{prefix}/child");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "edit_page",
        json!({ "path": path, "markdown": "findable content", "summary": "findme" }),
    )
    .await;
    assert!(!is_tool_error(&created), "edit_page failed: {created:?}");

    let found = call_tool(
        &fx.app,
        &fx.token,
        "search_pages",
        json!({ "prefix": prefix }),
    )
    .await;
    assert!(!is_tool_error(&found), "search_pages failed: {found:?}");
    let found_text = tool_text(&found);
    assert!(found_text.contains(&path), "search results: {found_text}");
    assert!(found_text.contains("total: 1"));

    let deleted = call_tool(&fx.app, &fx.token, "delete_page", json!({ "path": path })).await;
    assert!(!is_tool_error(&deleted));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn delete_page_on_unknown_path_is_a_tool_level_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "pages-delete-missing").await;
    let path = format!("mcp-test/never-created-{}", uuid::Uuid::new_v4());

    let deleted = call_tool(&fx.app, &fx.token, "delete_page", json!({ "path": path })).await;
    assert!(is_tool_error(&deleted));
    assert!(tool_text(&deleted).contains("Page not found"));

    cleanup_user(&fx.db, fx.user_id).await;
}
