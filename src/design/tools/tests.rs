use std::collections::HashSet;

use sea_orm::DatabaseConnection;
use serde_json::json;

use super::*;
use crate::routes::ws::Topic;

struct Fixture {
    dir: std::path::PathBuf,
    storage: Storage,
    design: DesignStore,
    hub: WsHub,
    db: DatabaseConnection,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("design-tools-{}", uuid::Uuid::new_v4()));
        let storage = Storage::local(&dir, DatabaseConnection::Disconnected).expect("fs storage");
        Self {
            dir,
            storage,
            design: DesignStore::new(None),
            hub: WsHub::new(),
            db: DatabaseConnection::Disconnected,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Output, ToolError> {
        let ctx = Ctx {
            db: &self.db,
            storage: &self.storage,
            design: &self.design,
            hub: &self.hub,
        };
        call(&ctx, name, args).await
    }

    async fn json(&self, name: &str, args: Value) -> Value {
        match self.call(name, args).await {
            Ok(Output::Json(v)) => v,
            Ok(Output::Text(t)) => panic!("{name}: expected JSON, got text {t:?}"),
            Err(e) => panic!("{name}: {e:?}"),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn specs_are_unique_design_tools_without_publish() {
    let mut names = HashSet::new();
    for spec in specs::TOOLS {
        assert!(spec.name.starts_with("design_"), "{}", spec.name);
        assert!(names.insert(spec.name), "duplicate {}", spec.name);
        assert_eq!(spec.schema()["type"], "object", "{}", spec.name);
        assert!(specs::find(spec.name).is_some());
    }
    assert_eq!(names.len(), 7);
    assert!(!names.iter().any(|n| n.contains("publish")));
}

#[test]
fn source_parses_the_three_views() {
    assert_eq!(Source::parse(None), Ok(Source::Draft));
    assert_eq!(Source::parse(Some("published")), Ok(Source::Published));
    assert_eq!(Source::parse(Some("baked")), Ok(Source::Baked));
    assert!(Source::parse(Some("live")).is_err());
}

#[test]
fn write_bytes_takes_exactly_one_encoding() {
    let args = |data: Option<&str>, b64: Option<&str>| WriteArgs {
        path: "assets/x".into(),
        data: data.map(String::from),
        data_base64: b64.map(String::from),
    };
    assert_eq!(write_bytes(&args(Some(""), None)), Ok(Bytes::new()));
    assert_eq!(
        write_bytes(&args(None, Some("AP8="))),
        Ok(Bytes::from_static(&[0, 0xff]))
    );
    assert!(write_bytes(&args(None, None)).is_err());
    assert!(write_bytes(&args(Some("a"), Some("AP8="))).is_err());
    assert!(write_bytes(&args(None, Some("not base64!"))).is_err());
    // Oversize base64 is refused by its length, before decoding.
    let huge = "A".repeat(MAX_FILE_SIZE.div_ceil(3) * 4 + 4);
    let err = write_bytes(&args(None, Some(&huge))).expect_err("too large");
    assert!(err.0.starts_with("file too large"), "{err:?}");
}

#[test]
fn storage_errors_stay_generic() {
    let outage = object_store::Error::Generic {
        store: "S3",
        source: "http://10.0.0.1:3900 refused".into(),
    };
    let err = DesignError::Storage(crate::storage::Error::Unavailable(outage));
    assert_eq!(
        ToolError::from(err),
        ToolError("storage unavailable".into())
    );
    let bad = ToolError::from(DesignError::BadPath("x".into()));
    assert!(bad.0.contains("templates/, assets/ or mdcast/"), "{bad:?}");
}

#[tokio::test]
async fn write_read_changes_delete_round_trip() {
    let fx = Fixture::new();
    let (_tx, mut rx) = fx.hub.register(1);

    let listed = fx
        .json(specs::LIST, json!({ "prefix": "templates/" }))
        .await;
    assert_eq!(listed["initialized"], false);
    let files = listed["files"].as_array().expect("files");
    assert!(files.iter().all(|f| {
        f["path"]
            .as_str()
            .is_some_and(|p| p.starts_with("templates/"))
    }));
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "templates/404.html" && f["baked"] == true)
    );

    let written = fx
        .json(
            specs::WRITE,
            json!({ "path": "templates/404.html", "data": "hi" }),
        )
        .await;
    assert_eq!(written["size"], 2);
    let event = rx.try_recv().expect("draft_changed");
    assert_eq!(
        (event.topic, event.event.as_str()),
        (Topic::Design, "draft_changed")
    );
    assert_eq!(
        event.payload,
        json!({ "action": "put", "path": "templates/404.html" })
    );

    let font = json!({ "path": "assets/fonts/x.woff2", "data_base64": "d09GMgD/" });
    fx.json(specs::WRITE, font).await;
    let read = fx
        .json(specs::READ, json!({ "path": "assets/fonts/x.woff2" }))
        .await;
    assert_eq!(read["data_base64"], "d09GMgD/");
    assert_eq!(read["mimetype"], "font/woff2");
    let read = fx
        .json(specs::READ, json!({ "path": "templates/404.html" }))
        .await;
    assert_eq!(read["data"], "hi");
    let baked = fx
        .json(
            specs::READ,
            json!({ "path": "templates/404.html", "source": "baked" }),
        )
        .await;
    assert_ne!(baked["data"], "hi");

    let changes = fx.json(specs::CHANGES, json!({})).await;
    assert_eq!(
        changes["changes"],
        json!([
            { "path": "assets/fonts/x.woff2", "kind": "added" },
            { "path": "templates/404.html", "kind": "modified" }
        ])
    );

    let Ok(Output::Text(reverted)) = fx
        .call(specs::DELETE, json!({ "path": "templates/404.html" }))
        .await
    else {
        panic!("delete failed");
    };
    assert!(reverted.contains("baked default"), "{reverted}");
    let gone = fx
        .call(specs::DELETE, json!({ "path": "templates/404.html" }))
        .await;
    assert!(matches!(gone, Err(ToolError(m)) if m.contains("not in the draft")));
    let changes = fx.json(specs::CHANGES, json!({})).await;
    assert_eq!(changes["changes"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn bad_paths_and_unknown_files_are_tool_errors() {
    let fx = Fixture::new();
    for path in ["../etc/passwd", "templates/", "other/x.css"] {
        let res = fx
            .call(specs::WRITE, json!({ "path": path, "data": "x" }))
            .await;
        assert!(res.is_err(), "{path}");
    }
    let missing = fx
        .call(specs::READ, json!({ "path": "assets/none.css" }))
        .await;
    assert_eq!(
        missing.err(),
        Some(ToolError("assets/none.css not found".into()))
    );
    let unknown = fx.call("design_publish", json!({})).await;
    assert!(unknown.is_err());
}

#[tokio::test]
async fn contract_is_markdown_or_schema() {
    let fx = Fixture::new();
    let Ok(Output::Text(md)) = fx.call(specs::CONTRACT, json!({})).await else {
        panic!("contract");
    };
    assert_eq!(md, contract::markdown());
    let Ok(Output::Text(schema)) = fx.call(specs::CONTRACT, json!({ "schema": true })).await else {
        panic!("schema");
    };
    assert!(serde_json::from_str::<Value>(&schema).is_ok());
}

#[tokio::test]
async fn tool_writes_reach_the_cached_draft_preview() {
    use crate::design::view::Resolve;

    let fx = Fixture::new();
    let site = || fx.design.draft_site(&fx.storage, true);
    let path = "assets/css/extra.css";
    assert!(site().await.expect("site").load(path).is_none());
    for data in ["a", "b"] {
        fx.json(specs::WRITE, json!({ "path": path, "data": data }))
            .await;
        let loaded = site().await.expect("site").load(path);
        assert_eq!(loaded.as_deref(), Some(data.as_bytes()));
    }
    assert!(
        fx.call(specs::DELETE, json!({ "path": path }))
            .await
            .is_ok()
    );
    assert!(site().await.expect("site").load(path).is_none());
}
