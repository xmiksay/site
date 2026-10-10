//! Chat attachments (#132): two concurrent uploads of one name both succeed
//! with distinct paths — a site file retries the next free name when it
//! loses the race on the unique `files.path` index, a draft asset is placed
//! under the draft lock (`DesignStore::draft_put_new`). Harness in
//! `common/attachments_app.rs`. Gated on `DATABASE_URL`.

#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

#[allow(dead_code)]
#[path = "common/attachments_app.rs"]
mod attachments_app;

use attachments_app::setup;
use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn concurrent_same_name_attachments_get_distinct_paths() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&url).await;
    let chat = fx.create("build").await;
    let name = format!("race-{}.txt", uuid::Uuid::new_v4());
    let (a, b) = tokio::join!(
        fx.attach(chat.id, &name, b"one"),
        fx.attach(chat.id, &name, b"two")
    );
    assert_eq!(
        (a.0, b.0),
        (StatusCode::CREATED, StatusCode::CREATED),
        "{a:?} {b:?}"
    );
    let files = [
        a.1["path"].as_str().expect("path"),
        b.1["path"].as_str().expect("path"),
    ];
    assert_ne!(files[0], files[1]);

    let designer = fx.create("designer").await;
    let (a, b) = tokio::join!(
        fx.attach(designer.id, "race.png", b"one"),
        fx.attach(designer.id, "race.png", b"two")
    );
    assert_eq!(
        (a.0, b.0),
        (StatusCode::CREATED, StatusCode::CREATED),
        "{a:?} {b:?}"
    );
    let mut assets = [a.1["path"].clone(), b.1["path"].clone()];
    assets.sort_by_key(|p| p.to_string());
    assert_eq!(
        assets,
        [json!("assets/img/race-2.png"), json!("assets/img/race.png")]
    );

    let paths = files.map(String::from);
    fx.cleanup(&paths).await;
}
