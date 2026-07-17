//! Integration coverage for the hand-rolled JSON-RPC MCP server at
//! `POST /mcp` (`src/routes/mcp/mod.rs`): auth, the `initialize`/
//! `notifications/initialized`/`tools/list` envelope methods, and the
//! `tools/call` dispatch error paths (unknown method/tool, missing params,
//! malformed arguments). Real per-tool round trips (`edit_page`+`read_page`,
//! `list_tags`+`create_tag`, `list_files`+`create_file`, ...) live in the
//! sibling `tests/mcp_pages.rs` and `tests/mcp_tags_files_galleries.rs` —
//! split by tool family the same way the production code itself is
//! (`src/routes/mcp/{pages,tags,files,galleries}.rs`), and to keep each file
//! under the repo's 400-line cap.
//!
//! DB-gated like every other integration test here (`eprintln!` + return
//! rather than `#[cfg]`, so `cargo test`/`make verify` stays green without a
//! live test DB) — see `tests/policy_db.rs`'s module doc for the full
//! throwaway-user-per-test + cascade-delete convention this reuses.

#[path = "common/mcp.rs"]
mod mcp_common;

use axum::http::StatusCode;
use mcp_common::{call_tool, cleanup_user, is_tool_error, rpc, setup, test_db_url, tool_text};
use serde_json::json;
use site::repo::pages::{self as pages_repo, PageNew};

#[tokio::test]
async fn missing_authorization_header_returns_401_with_www_authenticate() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "auth-missing").await;

    let (status, body, headers) = rpc(&fx.app, None, "initialize", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], json!(-32000));
    assert!(headers.contains_key("www-authenticate"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn garbage_bearer_token_returns_401() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "auth-garbage").await;

    let (status, body, headers) = rpc(&fx.app, Some("not-a-real-token"), "initialize", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], json!(-32000));
    let www_auth = headers
        .get("www-authenticate")
        .expect("missing WWW-Authenticate header")
        .to_str()
        .expect("header not ascii");
    assert!(www_auth.contains("invalid_token"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn valid_service_token_proceeds_to_the_handler() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "auth-valid").await;

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "initialize", None).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["error"].is_null(), "unexpected error: {body:?}");

    cleanup_user(&fx.db, fx.user_id).await;
}

// The two `initialize` scenarios below (no `CLAUDE` page vs. one present) are
// kept as a single sequential test rather than two `#[tokio::test]`s: the
// `CLAUDE` path is a global singleton (`src/repo/pages.rs`'s `path` is
// site-wide, not user-scoped), and `cargo test` runs test functions
// concurrently on separate threads within the same binary — two independent
// tests, one asserting the page is absent while the other creates/deletes it,
// would race.
#[tokio::test]
async fn initialize_reports_protocol_and_respects_the_claude_page_override() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "init").await;

    // No `CLAUDE` page exists yet, so this must be the built-in fallback,
    // which always documents the markdown directive set.
    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "initialize", None).await;
    assert_eq!(status, StatusCode::OK);
    let result = &body["result"];
    assert!(result["protocolVersion"].as_str().is_some());
    assert_eq!(result["serverInfo"]["name"], json!("site"));
    assert!(result["capabilities"]["tools"].is_object());
    let instructions = result["instructions"]
        .as_str()
        .expect("instructions should be a string");
    assert!(instructions.contains("Markdown extensions"));

    let custom_markdown = "# Custom instructions for this install\n\nDo the thing.";
    pages_repo::create(
        &fx.db,
        fx.user_id,
        PageNew {
            path: "CLAUDE".to_string(),
            markdown: custom_markdown.to_string(),
            summary: None,
            tag_ids: vec![],
            private: true,
        },
    )
    .await
    .expect("insert CLAUDE override page");

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "initialize", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["instructions"], json!(custom_markdown));

    pages_repo::delete_by_path(&fx.db, "CLAUDE")
        .await
        .expect("delete CLAUDE override page");
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn notifications_initialized_returns_empty_result() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "notif").await;

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "notifications/initialized", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!({}));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn tools_list_returns_the_known_tool_names() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "tools-list").await;

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "tools/list", None).await;

    assert_eq!(status, StatusCode::OK);
    let tools = body["result"]["tools"]
        .as_array()
        .expect("tools should be an array");
    assert!(!tools.is_empty());
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "read_page",
        "edit_page",
        "list_tags",
        "create_file",
        "list_galleries",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "unknown-method").await;

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "not/a/real/method", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!(-32601));
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Method not found")
    );

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn tools_call_without_params_returns_missing_params_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "missing-params").await;

    let (status, body, _) = rpc(&fx.app, Some(&fx.token), "tools/call", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!(-32602));
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Missing params")
    );

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn tools_call_unknown_tool_name_returns_unknown_tool_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "unknown-tool").await;

    let (status, body, _) = rpc(
        &fx.app,
        Some(&fx.token),
        "tools/call",
        Some(json!({ "name": "not_a_real_tool", "arguments": {} })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!(-32602));
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Unknown tool: not_a_real_tool")
    );

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn edit_page_missing_required_path_is_a_tool_level_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "malformed-missing").await;

    // `path` is required by `EditPageArgs` — omitting it should fail
    // `parse_args`, not panic or 500.
    let resp = call_tool(
        &fx.app,
        &fx.token,
        "edit_page",
        json!({ "summary": "no path here" }),
    )
    .await;

    assert!(is_tool_error(&resp));
    assert!(tool_text(&resp).starts_with("Invalid arguments:"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn edit_page_wrong_type_for_a_field_is_a_tool_level_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "malformed-type").await;

    // `path` must be a string; sending a number should fail deserialization
    // cleanly rather than panicking.
    let resp = call_tool(&fx.app, &fx.token, "edit_page", json!({ "path": 12345 })).await;

    assert!(is_tool_error(&resp));
    assert!(tool_text(&resp).starts_with("Invalid arguments:"));

    cleanup_user(&fx.db, fx.user_id).await;
}
