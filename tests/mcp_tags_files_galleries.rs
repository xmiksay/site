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
async fn tag_list_create_read_delete_round_trip() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "tags-roundtrip").await;
    let name = format!("mcp-test-tag-{}", uuid::Uuid::new_v4());

    let before = call_tool(&fx.app, &fx.token, "tag_list", json!({})).await;
    assert!(!is_tool_error(&before), "tag_list failed: {before:?}");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "tag_create",
        json!({ "name": name, "description": "a throwaway test tag" }),
    )
    .await;
    assert!(!is_tool_error(&created), "tag_create failed: {created:?}");
    assert!(tool_text(&created).contains(&name));

    let read = call_tool(&fx.app, &fx.token, "tag_read", json!({ "name": name })).await;
    assert!(!is_tool_error(&read), "tag_read failed: {read:?}");
    let read_json = tool_json(&read);
    assert_eq!(read_json["name"], json!(name));
    assert_eq!(read_json["description"], json!("a throwaway test tag"));

    let after = call_tool(&fx.app, &fx.token, "tag_list", json!({})).await;
    assert!(tool_text(&after).contains(&name));

    let deleted = call_tool(&fx.app, &fx.token, "tag_delete", json!({ "name": name })).await;
    assert!(!is_tool_error(&deleted), "tag_delete failed: {deleted:?}");
    assert!(tool_text(&deleted).contains(&name));

    let read_after_delete =
        call_tool(&fx.app, &fx.token, "tag_read", json!({ "name": name })).await;
    assert!(is_tool_error(&read_after_delete));
    assert!(tool_text(&read_after_delete).contains("Tag not found"));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn file_list_file_create_and_file_read_round_trip() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "files-roundtrip").await;
    let path = format!("mcp-test-file-{}.txt", uuid::Uuid::new_v4());
    let payload = base64::engine::general_purpose::STANDARD.encode(b"tiny test payload");

    let before = call_tool(&fx.app, &fx.token, "file_list", json!({})).await;
    assert!(!is_tool_error(&before), "file_list failed: {before:?}");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "file_create",
        json!({
            "path": path,
            "description": "a throwaway test file",
            "mimetype": "text/plain",
            "data_base64": payload,
        }),
    )
    .await;
    assert!(!is_tool_error(&created), "file_create failed: {created:?}");
    let created_json = tool_json(&created);
    assert_eq!(created_json["path"], json!(path));
    assert_eq!(created_json["mimetype"], json!("text/plain"));
    assert_eq!(
        created_json["embed"],
        json!(format!("<file id=\"{}\">", created_json["id"]))
    );
    let file_id = created_json["id"].as_i64().expect("created file id");

    let read = call_tool(&fx.app, &fx.token, "file_read", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&read), "file_read failed: {read:?}");
    let read_json = tool_json(&read);
    assert_eq!(read_json["path"], json!(path));
    assert_eq!(read_json["description"], json!("a throwaway test file"));

    let deleted = call_tool(&fx.app, &fx.token, "file_delete", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted), "file_delete failed: {deleted:?}");

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn file_update_replaces_content_and_file_read_returns_it() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "files-repair").await;
    let path = format!("mcp-test-repair-{}.txt", uuid::Uuid::new_v4());
    let original = base64::engine::general_purpose::STANDARD.encode(b"original content");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "file_create",
        json!({
            "path": path,
            "mimetype": "text/plain",
            "data_base64": original,
        }),
    )
    .await;
    assert!(!is_tool_error(&created), "file_create failed: {created:?}");
    let file_id = tool_json(&created)["id"].as_i64().expect("created file id");

    // Repair: overwrite the file's content in place at the same id/path.
    let new_content = "repaired content, much longer than the original";
    let updated = call_tool(
        &fx.app,
        &fx.token,
        "file_update",
        json!({
            "id": file_id,
            "path": path,
            "mimetype": "text/plain",
            "data": new_content,
        }),
    )
    .await;
    assert!(!is_tool_error(&updated), "file_update failed: {updated:?}");

    let read_with_content = call_tool(
        &fx.app,
        &fx.token,
        "file_read",
        json!({ "id": file_id, "include_content": true }),
    )
    .await;
    assert!(
        !is_tool_error(&read_with_content),
        "file_read failed: {read_with_content:?}"
    );
    let read_json = tool_json(&read_with_content);
    assert_eq!(read_json["content"], json!(new_content));
    assert_eq!(read_json["size_bytes"], json!(new_content.len() as i64));

    // Back-compat: omitting include_content must not add a "content" field.
    let read_without_content =
        call_tool(&fx.app, &fx.token, "file_read", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&read_without_content));
    assert!(tool_json(&read_without_content).get("content").is_none());

    let deleted = call_tool(&fx.app, &fx.token, "file_delete", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted), "file_delete failed: {deleted:?}");

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn file_create_hints_the_type_specific_embed_directive() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "files-embed-hint").await;
    let path = format!("mcp-test-game-{}.pgn", uuid::Uuid::new_v4());
    let payload = base64::engine::general_purpose::STANDARD.encode(b"1. e4 e5 2. Nf3 Nc6");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "file_create",
        json!({ "path": path, "data_base64": payload }),
    )
    .await;
    assert!(!is_tool_error(&created), "file_create failed: {created:?}");
    let created_json = tool_json(&created);
    let file_id = created_json["id"].as_i64().expect("created file id");
    assert_eq!(
        created_json["embed"],
        json!(format!("<pgn id=\"{file_id}\">")),
        "a .pgn upload must hint <pgn>, not <image> (issue #55)"
    );

    let deleted = call_tool(&fx.app, &fx.token, "file_delete", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn file_create_without_mimetype_infers_it_from_the_extension() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "files-infer-mimetype").await;
    let path = format!("mcp-test-game-{}.pgn", uuid::Uuid::new_v4());
    let payload = base64::engine::general_purpose::STANDARD.encode(b"1. e4 e5 2. Nf3 Nc6");

    let created = call_tool(
        &fx.app,
        &fx.token,
        "file_create",
        json!({ "path": path, "data_base64": payload }),
    )
    .await;
    assert!(!is_tool_error(&created), "file_create failed: {created:?}");
    let created_json = tool_json(&created);
    assert_eq!(
        created_json["mimetype"],
        json!("application/x-chess-pgn"),
        "a .pgn upload without an explicit mimetype must not land as octet-stream (issue #57)"
    );
    let file_id = created_json["id"].as_i64().expect("created file id");

    let deleted = call_tool(&fx.app, &fx.token, "file_delete", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted));

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn gallery_list_and_gallery_create_round_trip() {
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
        "file_create",
        json!({ "path": file_path, "data_base64": payload }),
    )
    .await;
    assert!(!is_tool_error(&file), "file_create failed: {file:?}");
    let file_id = tool_json(&file)["id"].as_i64().expect("created file id");

    let before = call_tool(&fx.app, &fx.token, "gallery_list", json!({})).await;
    assert!(!is_tool_error(&before), "gallery_list failed: {before:?}");

    let gallery_path = format!("mcp-test-gallery-{}", uuid::Uuid::new_v4());
    let created = call_tool(
        &fx.app,
        &fx.token,
        "gallery_create",
        json!({
            "path": gallery_path,
            "title": "a throwaway test gallery",
            "file_ids": [file_id],
        }),
    )
    .await;
    assert!(
        !is_tool_error(&created),
        "gallery_create failed: {created:?}"
    );
    assert!(tool_text(&created).contains("a throwaway test gallery"));

    let after = call_tool(&fx.app, &fx.token, "gallery_list", json!({})).await;
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
        "gallery_delete",
        json!({ "id": gallery_id }),
    )
    .await;
    assert!(!is_tool_error(&deleted_gallery));

    let deleted_file = call_tool(&fx.app, &fx.token, "file_delete", json!({ "id": file_id })).await;
    assert!(!is_tool_error(&deleted_file));

    cleanup_user(&fx.db, fx.user_id).await;
}
