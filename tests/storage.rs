//! Blob storage (#109): one suite over every backend — `db`, `fs` and `s3`
//! (a random prefix in the `TEST_S3_*` bucket) — plus `storage migrate` and
//! the public `/files/{hash}` route on top of a non-db backend.
//!
//! Gated on `DATABASE_URL` like every DB test (every backend keeps its
//! `file_blobs` rows in Postgres). The S3 tests then *fail*, not skip,
//! without `TEST_S3_*` — see `common/storage.rs`.

#[path = "common/storage.rs"]
mod storage_fixture;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::TryStreamExt as _;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::config::Config;
use site::entity::{file, file_blob, user};
use site::files::hash_blob;
use site::repo::files::{self as files_repo, FileSaveError, NewFile};
use site::storage::{Error, Storage, StorageConfig, migrate};
use storage_fixture::{TestStorage, dead_s3_config};
use tower::ServiceExt;

fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

async fn test_db() -> Option<DatabaseConnection> {
    let url = test_db_url()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn unique_bytes(tag: &str) -> Vec<u8> {
    format!("{tag} {}", uuid::Uuid::new_v4()).into_bytes()
}

async fn blob_row(db: &DatabaseConnection, hash: &str) -> Option<file_blob::Model> {
    file_blob::Entity::find_by_id(hash.to_string())
        .one(db)
        .await
        .expect("query file_blobs")
}

async fn delete_blob_rows(db: &DatabaseConnection, hashes: &[String]) {
    for hash in hashes {
        file_blob::Entity::delete_by_id(hash.clone())
            .exec(db)
            .await
            .expect("delete throwaway file_blob");
    }
}

/// The contract every backend honors.
async fn exercise(db: &DatabaseConnection, ts: &TestStorage) {
    let storage = &ts.storage;
    let data = unique_bytes(storage.kind());

    let hash = storage.put_blob(&data).await.expect("put_blob");
    assert_eq!(hash, hash_blob(&data));
    assert_eq!(
        storage.put_blob(&data).await.expect("idempotent re-put"),
        hash
    );

    let got = storage.get_blob(&hash).await.expect("get_blob");
    assert_eq!(got.as_deref(), Some(data.as_slice()));

    let download = storage
        .get_blob_stream(&hash)
        .await
        .expect("get_blob_stream")
        .expect("streamed blob present");
    assert_eq!(download.size, data.len() as u64);
    let streamed: Vec<u8> = download
        .stream
        .map_ok(|chunk| chunk.to_vec())
        .try_concat()
        .await
        .expect("stream body");
    assert_eq!(streamed, data);

    // Every backend records the blob; only `db` keeps the bytes in the row.
    let row = blob_row(db, &hash).await.expect("file_blobs row");
    assert_eq!(row.size_bytes, data.len() as i64);
    let expected = (storage.kind() == "db").then(|| data.clone());
    assert_eq!(row.data, expected, "{} backend", storage.kind());

    let missing = hash_blob(&unique_bytes("never stored"));
    assert!(storage.get_blob(&missing).await.expect("get").is_none());
    assert!(
        storage
            .get_blob_stream(&missing)
            .await
            .expect("stream")
            .is_none()
    );

    delete_blob_rows(db, &[hash]).await;
}

#[tokio::test]
async fn db_backend() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::db(&db)).await;
}

#[tokio::test]
async fn fs_backend() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn s3_backend() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    exercise(&db, &TestStorage::s3(&db)).await;
}

#[tokio::test]
async fn db_backend_fills_a_metadata_only_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fs = TestStorage::fs(&db);
    let data = unique_bytes("back to db");
    let hash = fs.storage.put_blob(&data).await.expect("fs put");
    assert_eq!(blob_row(&db, &hash).await.expect("row").data, None);

    let db_storage = Storage::db(db.clone());
    db_storage.put_blob(&data).await.expect("db put");
    assert_eq!(blob_row(&db, &hash).await.expect("row").data, Some(data));

    delete_blob_rows(&db, &[hash]).await;
}

#[tokio::test]
async fn unreachable_s3_is_unavailable_and_records_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let dead = Storage::s3(&dead_s3_config(), db.clone()).expect("dead s3");
    let data = unique_bytes("dead");
    let err = dead.put_blob(&data).await.expect_err("dead put");
    assert!(matches!(err, Error::Unavailable(_)), "{err:?}");
    assert!(blob_row(&db, &hash_blob(&data)).await.is_none());

    let user_id = throwaway_user(&db, "dead-s3").await;
    let err = files_repo::create_file(
        &db,
        &dead,
        user_id,
        NewFile {
            path: format!("storage-test/{}", uuid::Uuid::new_v4()),
            description: None,
            mimetype: "text/plain".into(),
            data,
        },
    )
    .await
    .err()
    .expect("create_file over dead storage");
    assert!(
        matches!(err, FileSaveError::Storage(Error::Unavailable(_))),
        "{err:?}"
    );
    user::Entity::delete_by_id(user_id)
        .exec(&db)
        .await
        .expect("delete throwaway user");
}

async fn migrate_from_db_into(db: &DatabaseConnection, target: &TestStorage) {
    // Scoped: an unscoped db source would also copy other tests' keyed objects.
    let source_ts = TestStorage::db(db);
    let source = &source_ts.storage;
    let a = source
        .put_blob(&unique_bytes("migrate a"))
        .await
        .expect("a");
    let b = source
        .put_blob(&unique_bytes("migrate b"))
        .await
        .expect("b");
    let hashes = vec![a.clone(), b.clone()];

    let report = migrate::migrate(source, &target.storage, &hashes)
        .await
        .expect("migrate");
    assert_eq!(report.blobs.copied, hashes, "{report:?}");
    assert_eq!(report.problems().count(), 0, "{report:?}");
    for hash in &hashes {
        let got = target.storage.get_blob(hash).await.expect("get");
        assert_eq!(got, source.get_blob(hash).await.expect("source get"));
    }
    // The source is only read.
    assert!(blob_row(db, &a).await.expect("row").data.is_some());

    let again = migrate::migrate(source, &target.storage, &hashes)
        .await
        .expect("re-run");
    assert_eq!(again.blobs.present, hashes, "re-run is a no-op: {again:?}");
    assert!(again.blobs.copied.is_empty() && again.problems().count() == 0);

    let unknown = hash_blob(&unique_bytes("nowhere"));
    let missing = migrate::migrate(source, &target.storage, std::slice::from_ref(&unknown))
        .await
        .expect("missing source");
    assert_eq!(missing.blobs.problems.len(), 1, "{missing:?}");
    assert!(missing.blobs.problems[0].contains("missing in the source"));

    delete_blob_rows(db, &hashes).await;
}

#[tokio::test]
async fn migrate_db_to_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    migrate_from_db_into(&db, &TestStorage::fs(&db)).await;
}

#[tokio::test]
async fn migrate_db_to_s3() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    migrate_from_db_into(&db, &TestStorage::s3(&db)).await;
}

#[tokio::test]
async fn migrate_keeps_differing_target_content_and_refuses_db_to_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let source_ts = TestStorage::db(&db);
    let source = &source_ts.storage;
    let data = unique_bytes("differs");
    let hash = source.put_blob(&data).await.expect("put");

    let target = TestStorage::fs(&db);
    let StorageConfig::Fs { dir } = target.fs_config() else {
        unreachable!("fs_config is always Fs");
    };
    let path = dir.join("blobs").join(&hash[..2]).join(&hash);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, b"tampered").expect("write tampered object");

    let hashes = vec![hash.clone()];
    let report = migrate::migrate(source, &target.storage, &hashes)
        .await
        .expect("migrate");
    assert_eq!(report.blobs.problems.len(), 1, "{report:?}");
    assert!(report.blobs.problems[0].contains("different content"));
    assert_eq!(std::fs::read(&path).expect("read"), b"tampered");

    let err = migrate::migrate(source, &Storage::db(db.clone()), &hashes)
        .await
        .expect_err("db → db");
    assert!(err.to_string().contains("both the database"), "{err}");

    delete_blob_rows(&db, &hashes).await;
}

async fn throwaway_user(db: &DatabaseConnection, tag: &str) -> i32 {
    user::ActiveModel {
        username: Set(format!("storage-{tag}-{}", uuid::Uuid::new_v4())),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user")
    .id
}

/// `GET /files/{hash}` (and its thumbnail) through the public router over a
/// full `AppState` whose blobs come from `storage`. The state starts on `db`
/// and gets `storage` swapped in: startup itself refuses an unreachable
/// storage (design overrides), and this tests serving, not startup.
async fn get_public(storage: Storage, uri: &str) -> (StatusCode, Vec<u8>) {
    let config = Config {
        database_url: test_db_url().expect("DATABASE_URL"),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage: StorageConfig::Db,
    };
    let mut state = site::state::create_state(&config).await;
    state.storage = storage;
    let app = axum::Router::new()
        .nest("/files", site::routes::public::images::router())
        .with_state(state);
    let req = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("response");
    let status = resp.status();
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn public_route_serves_from_fs_and_answers_503_when_s3_is_down() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let user_id = throwaway_user(&db, "public").await;
    // Random pixels: both the image and its JPEG thumbnail hash are this
    // run's own, so cleanup never races another run over a shared blob.
    let png = {
        let seed = uuid::Uuid::new_v4().into_bytes();
        let mut buf = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_fn(4, 4, |x, y| {
            let i = (y * 4 + x) as usize;
            image::Rgb([seed[i], seed[(i + 5) % 16], seed[(i + 11) % 16]])
        })
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("encode png");
        buf.into_inner()
    };
    let created = files_repo::create_file(
        &db,
        &ts.storage,
        user_id,
        NewFile {
            path: format!("storage-test/{}.png", uuid::Uuid::new_v4()),
            description: None,
            mimetype: "image/png".into(),
            data: png.clone(),
        },
    )
    .await
    .expect("create_file over fs");
    assert!(created.has_thumbnail, "thumbnail stored through fs too");
    let hash = created.model.hash.clone();

    let (status, body) = get_public(ts.storage.clone(), &format!("/files/{hash}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, png);
    let (status, thumb) = get_public(ts.storage.clone(), &format!("/files/{hash}/nahled")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!thumb.is_empty());

    let (status, _) = get_public(
        Storage::s3(&dead_s3_config(), db.clone()).expect("dead s3"),
        &format!("/files/{hash}"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // A row whose blob the backend lacks (another, empty dir) is a 404.
    let empty = TestStorage::fs(&db);
    let (status, _) = get_public(empty.storage.clone(), &format!("/files/{hash}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let thumb_hash = site::entity::file_thumbnail::Entity::find_by_id(created.model.id)
        .one(&db)
        .await
        .expect("thumbnail row")
        .map(|t| t.hash);
    // Cascades to the file_thumbnails row.
    file::Entity::delete_by_id(created.model.id)
        .exec(&db)
        .await
        .expect("delete file");
    let mut hashes = vec![hash];
    hashes.extend(thumb_hash);
    delete_blob_rows(&db, &hashes).await;
    user::Entity::delete_by_id(user_id)
        .exec(&db)
        .await
        .expect("delete throwaway user");
}
