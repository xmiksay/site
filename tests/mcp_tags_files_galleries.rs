//! `tools/call` round trips for the `tags`, `files` and `galleries` tool
//! families (`src/routes/mcp/{tags,files,galleries}.rs`) through the real
//! `POST /mcp` handler. Pages round trips and envelope-level scenarios live
//! in the sibling `tests/mcp_pages.rs` / `tests/mcp_endpoint.rs`; shared
//! fixture/helpers in `tests/common/mcp.rs`. Same DB-gated + throwaway-user
//! convention as `tests/policy_db.rs`.

#[path = "common/mcp.rs"]
mod mcp_common;

use base64::Engine;
use mcp_common::{
    call_tool, cleanup_user, is_tool_error, setup, test_db_url, tool_json, tool_text,
};
use serde_json::json;

#[tokio::test]
async fn list_tags_create_tag_read_tag_and_delete_tag_round_trip() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "tags-roundtrip").await;
    let name = format!("mcp-test-tag-{}", uuid::Uuid::new_v4());

    let before = call_tool(&fx.app, &fx.token, "list_tags", json!({})).await;
    assert!(!is_tool_error(&before), "list_tags failed: {before:?}");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "create_tag",
        json!({ "name": name, "description": "a throwaway test tag" }),
    )
    .await;
    assert!(!is_tool_error(&created), "create_tag failed: {created:?}");
    assert!(tool_text(&created).contains(&name));

    let read = call_tool(&fx.app, &fx.token, "read_tag", json!({ "name": name })).await;
    assert!(!is_tool_error(&read), "read_tag failed: {read:?}");
    let read_json = tool_json(&read);
    assert_eq!(read_json["name"], json!(name));
    assert_eq!(read_json["description"], json!("a throwaway test tag"));

    let after = call_tool(&fx.app, &fx.token, "list_tags", json!({})).await;
    assert!(tool_text(&after).contains(&name));

    let deleted = call_tool(&fx.app, &fx.token, "delete_tag", json!({ "name": name })).await;
    assert!(!is_tool_error(&deleted), "delete_tag failed: {deleted:?}");
    assert!(tool_text(&deleted).contains(&name));

    let read_after_delete =
        call_tool(&fx.app, &fx.token, "read_tag", json!({ "name": name })).await;
    assert!(is_tool_error(&read_after_delete));
    assert!(tool_text(&read_after_delete).contains("Tag not found"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn list_files_create_file_and_read_file_round_trip() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "files-roundtrip").await;
    let path = format!("mcp-test-file-{}.txt", uuid::Uuid::new_v4());
    let payload = base64::engine::general_purpose::STANDARD.encode(b"tiny test payload");

    let before = call_tool(&fx.app, &fx.token, "list_files", json!({})).await;
    assert!(!is_tool_error(&before), "list_files failed: {before:?}");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "create_file",
        json!({
            "path": path,
            "description": "a throwaway test file",
            "mimetype": "text/plain",
            "data_base64": payload,
        }),
    )
    .await;
    assert!(!is_tool_error(&created), "create_file failed: {created:?}");
    let created_json = tool_json(&created);
    assert_eq!(created_json["path"], json!(path));
    assert_eq!(created_json["mimetype"], json!("text/plain"));
    let file_id = created_json["id"].as_i64().expect("created file id");

    let read = call_tool(&fx.app, &fx.token, "read_file", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&read), "read_file failed: {read:?}");
    let read_json = tool_json(&read);
    assert_eq!(read_json["path"], json!(path));
    assert_eq!(read_json["description"], json!("a throwaway test file"));

    let deleted = call_tool(&fx.app, &fx.token, "delete_file", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted), "delete_file failed: {deleted:?}");

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn list_galleries_and_create_gallery_round_trip() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "galleries-roundtrip").await;
    let file_path = format!("mcp-test-gallery-file-{}.txt", uuid::Uuid::new_v4());
    let payload = base64::engine::general_purpose::STANDARD.encode(b"gallery member payload");

    let file = call_tool(
        &fx.app,
        &fx.token,
        "create_file",
        json!({ "path": file_path, "data_base64": payload }),
    )
    .await;
    assert!(!is_tool_error(&file), "create_file failed: {file:?}");
    let file_id = tool_json(&file)["id"].as_i64().expect("created file id");

    let before = call_tool(&fx.app, &fx.token, "list_galleries", json!({})).await;
    assert!(!is_tool_error(&before), "list_galleries failed: {before:?}");

    let gallery_path = format!("mcp-test-gallery-{}", uuid::Uuid::new_v4());
    let created = call_tool(
        &fx.app,
        &fx.token,
        "create_gallery",
        json!({
            "path": gallery_path,
            "title": "a throwaway test gallery",
            "file_ids": [file_id],
        }),
    )
    .await;
    assert!(
        !is_tool_error(&created),
        "create_gallery failed: {created:?}"
    );
    assert!(tool_text(&created).contains("a throwaway test gallery"));

    let after = call_tool(&fx.app, &fx.token, "list_galleries", json!({})).await;
    assert!(tool_text(&after).contains("a throwaway test gallery"));

    // Extract the gallery id from `created gallery [ID] title` to clean up.
    let created_text = tool_text(&created);
    let gallery_id: i64 = created_text
        .split('[')
        .nth(1)
        .and_then(|s| s.split(']').next())
        .and_then(|s| s.parse().ok())
        .expect("parse gallery id from tool_result text");

    let deleted_gallery = call_tool(
        &fx.app,
        &fx.token,
        "delete_gallery",
        json!({ "id": gallery_id }),
    )
    .await;
    assert!(!is_tool_error(&deleted_gallery));

    let deleted_file = call_tool(&fx.app, &fx.token, "delete_file", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted_file));

    cleanup_user(&fx.db, fx.user_id).await;
}
