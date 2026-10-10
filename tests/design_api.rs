//! `/api/design` (#110, #115) over a full `AppState`: the draft API (tree +
//! changes, raw text and binary files, delete, discard), publish (422 on a
//! template that fails to compile or the strict smoke render, live
//! untouched), history and restore, the WS
//! `design.*` events, and Reload after a bucket edit — all driving what the
//! public 404 page renders, over fs and db storage.
//!
//! Gated on `DATABASE_URL` like every DB test.

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

#[path = "common/design_app.rs"]
mod design_app;

use axum::http::StatusCode;
use design_app::{App, app, entry, next_design_event, test_db};
use serde_json::json;
use site::storage::StorageConfig;
use storage_fixture::TestStorage;

async fn exercise(app: &App, ts: &TestStorage, kind: &str) {
    let (_tx, mut rx) = app.hub.register(app.user_id);
    let marker = format!("DESIGN-{}", uuid::Uuid::new_v4());
    let baked_404 = app.public_404().await;

    let (status, state) = app.json("GET", "/api/design/draft", "").await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["storage"], kind);
    assert_eq!(state["changes"], json!([]));
    assert_eq!(state["initialized"], false, "a GET never initializes");
    assert_eq!(entry(&state, "templates/404.html")["baked"], true);
    assert_eq!(entry(&state, "templates/404.html")["overridden"], false);

    let (status, state) = app
        .json("PUT", "/api/design/draft/templates/404.html", &marker)
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["initialized"], true);
    assert_eq!(entry(&state, "templates/404.html")["overridden"], true);
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }])
    );
    assert_eq!(
        next_design_event(&mut rx),
        (
            "draft_changed".into(),
            json!({ "action": "put", "path": "templates/404.html" })
        )
    );
    assert_eq!(app.public_404().await, baked_404, "invisible until publish");
    let read = |source: &'static str| async move {
        let uri = format!("/api/design/draft/templates/404.html{source}");
        let (status, body) = app.call("GET", &uri, "").await;
        assert_eq!(status, StatusCode::OK, "{source}");
        String::from_utf8_lossy(&body).into_owned()
    };
    assert_eq!(read("").await, marker);
    assert_eq!(read("?source=published").await, read("?source=baked").await);

    // Raw binary round trip, then delete.
    let png = vec![0x89, b'P', b'N', b'G', 0, 0xff];
    let (status, _) = app
        .call("PUT", "/api/design/draft/assets/img/x.png", png.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = app
        .call("GET", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(body, png);
    let (status, _) = app
        .call("DELETE", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .call("DELETE", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let events: Vec<_> = (0..2)
        .map(|_| next_design_event(&mut rx).1["action"].clone())
        .collect();
    assert_eq!(events, [json!("put"), json!("delete")]);

    // A broken template is accepted in the draft but blocks the publish.
    let (status, _) = app
        .call("PUT", "/api/design/draft/templates/404.html", "{% for %}")
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("templates/404.html"),
        "{body}"
    );
    assert_eq!(body["code"], "invalid", "{body}");
    let details = body["details"].as_array().expect("details");
    assert!(
        details.len() == 1
            && details[0]
                .as_str()
                .unwrap_or("")
                .starts_with("templates/404.html"),
        "{body}"
    );
    assert_eq!(app.public_404().await, baked_404, "live untouched");
    assert!(
        ts.storage
            .get("design/templates/404.html")
            .await
            .expect("get")
            .is_none()
    );

    let (status, _) = app
        .call(
            "PUT",
            "/api/design/draft/templates/404.html",
            marker.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // So does one that compiles but fails the strict smoke render, here in
    // a branch only the contract's example contexts reach (a search query).
    let search = "/api/design/draft/templates/page_search.html";
    let baked = site::design::DesignStore::new(None).baked("templates/page_search.html");
    let baked = String::from_utf8(baked.expect("baked page_search")).expect("utf-8");
    let broken = baked.replacen("{% elif q %}{{ q }}", "{% elif q %}{{ qq }}", 1);
    assert_eq!(app.call("PUT", search, broken).await.0, StatusCode::OK);
    let (status, body) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let error = body["error"].as_str().unwrap_or("");
    assert!(
        error.contains("templates/page_search.html:2: undefined"),
        "{body}"
    );
    assert_eq!(body["code"], "invalid", "{body}");
    assert!(
        body["details"][0]
            .as_str()
            .unwrap_or("")
            .starts_with("templates/page_search.html:2: undefined"),
        "{body}"
    );
    assert_eq!(app.public_404().await, baked_404, "live untouched");
    assert_eq!(app.call("DELETE", search, "").await.0, StatusCode::OK);

    let (status, first) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["by"], app.username.as_str());
    assert_eq!(app.public_404().await, marker);
    while let Ok(e) = rx.try_recv() {
        if e.event == "published" {
            assert_eq!(e.payload["id"], first["id"]);
        }
    }

    let (_, _) = app
        .call("PUT", "/api/design/draft/templates/404.html", "SECOND")
        .await;
    let (status, _) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.public_404().await, "SECOND");
    let (status, history) = app.json("GET", "/api/design/history", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().map(Vec::len), Some(2), "{history}");
    assert_eq!(history[1], first, "newest first");

    while rx.try_recv().is_ok() {}
    let id = first["id"].as_str().expect("id");
    let (status, state) = app
        .json("POST", &format!("/api/design/history/{id}/restore"), "")
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }])
    );
    assert_eq!(
        app.public_404().await,
        "SECOND",
        "restore goes to the draft"
    );
    assert_eq!(
        next_design_event(&mut rx),
        (
            "draft_changed".into(),
            json!({ "action": "restore", "version": id })
        )
    );

    let (status, state) = app.json("POST", "/api/design/draft/discard", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["changes"], json!([]));
    assert_eq!(next_design_event(&mut rx).1, json!({ "action": "discard" }));
    let (status, body) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("nothing to publish"),
        "{body}"
    );
    assert_eq!(body["code"], "nothing_to_publish", "{body}");
    assert!(body.get("details").is_none(), "{body}");

    let (status, _) = app
        .call(
            "POST",
            "/api/design/history/2001-01-01T00:00:00Z/restore",
            "",
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for bad in ["preview/x.html", "templates/..%2Fx"] {
        let (status, _) = app
            .call("PUT", &format!("/api/design/draft/{bad}"), "x")
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // Edited in the storage directly, picked up by Reload.
    ts.storage
        .put("design/templates/404.html", "EXTERNAL".into())
        .await
        .expect("external edit");
    let (status, state) = app.json("POST", "/api/design/reload", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["last_reload"]["ok"], true);
    assert_eq!(app.public_404().await, "EXTERNAL");
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }]),
        "the draft still holds the last published version"
    );
    // Publishing it would revert the bucket edit: 409 unless forced.
    let (status, body) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("changed outside the draft since it was started: templates/404.html"),
        "{body}"
    );
    assert_eq!(body["code"], "conflict", "{body}");
    assert_eq!(body["details"], json!(["templates/404.html"]), "{body}");
    assert_eq!(app.public_404().await, "EXTERNAL");
    let (status, _) = app.json("POST", "/api/design/publish?force=true", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.public_404().await, "SECOND");
}

#[tokio::test]
async fn design_api_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let app = app(ts.fs_config(), None).await;
    exercise(&app, &ts, "fs").await;
    app.cleanup().await;
}

#[tokio::test]
async fn design_api_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::db(&db);
    let app = app(StorageConfig::Db, Some(ts.storage.clone())).await;
    exercise(&app, &ts, "db").await;
    app.cleanup().await;
}
