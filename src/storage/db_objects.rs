//! Keyed objects of the `db` backend: one `storage_objects` row (m_034) per
//! key, `etag` being the sha256 of `data`. Keys arrive already validated and
//! prefixed by [`Storage`](super::Storage).

use bytes::Bytes;
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait as _, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
};

use super::objects::{Object, Version};
use crate::files::hash_blob;

fn stmt<const N: usize>(sql: &str, values: [sea_orm::Value; N]) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// A single upsert, so readers see the old row or the new one.
pub(super) async fn put(db: &DatabaseConnection, key: &str, data: &[u8]) -> Result<(), DbErr> {
    db.execute(stmt(
        "INSERT INTO storage_objects (key, data, etag, updated_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (key) DO UPDATE
         SET data = EXCLUDED.data, etag = EXCLUDED.etag, updated_at = EXCLUDED.updated_at",
        [key.into(), data.to_vec().into(), hash_blob(data).into()],
    ))
    .await
    .map(|_| ())
}

pub(super) async fn get(db: &DatabaseConnection, key: &str) -> Result<Option<Bytes>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        data: Vec<u8>,
    }
    let row = Row::find_by_statement(stmt(
        "SELECT data FROM storage_objects WHERE key = $1",
        [key.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(|r| Bytes::from(r.data)))
}

pub(super) async fn delete(db: &DatabaseConnection, key: &str) -> Result<(), DbErr> {
    db.execute(stmt(
        "DELETE FROM storage_objects WHERE key = $1",
        [key.into()],
    ))
    .await
    .map(|_| ())
}

/// Every row whose key starts with `prefix` (empty: all), unsorted, without
/// the data.
pub(super) async fn list(db: &DatabaseConnection, prefix: &str) -> Result<Vec<Object>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        key: String,
        etag: String,
        size: i64,
        updated_at: DateTime<Utc>,
    }
    // `starts_with`, not LIKE: `_` and `%` are ordinary key characters.
    let rows = Row::find_by_statement(stmt(
        "SELECT key, etag, octet_length(data)::bigint AS size, updated_at
         FROM storage_objects WHERE starts_with(key, $1)",
        [prefix.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Object {
            key: r.key,
            version: Version {
                e_tag: Some(r.etag),
                size: r.size.try_into().unwrap_or_default(),
                last_modified: r.updated_at,
            },
        })
        .collect())
}
