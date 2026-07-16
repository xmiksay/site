use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use sha2::{Digest, Sha256};

pub fn hash_blob(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub async fn put_blob<C: ConnectionTrait>(db: &C, hash: &str, data: &[u8]) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO file_blobs (hash, data, size_bytes) VALUES ($1, $2, $3) ON CONFLICT (hash) DO NOTHING",
        [hash.into(), data.into(), (data.len() as i64).into()],
    );
    db.execute(stmt).await?;
    Ok(())
}

pub async fn read_blob<C: ConnectionTrait>(db: &C, hash: &str) -> Result<Option<Vec<u8>>, DbErr> {
    use sea_orm::FromQueryResult;

    #[derive(FromQueryResult)]
    struct BlobRow {
        data: Vec<u8>,
    }

    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT data FROM file_blobs WHERE hash = $1",
        [hash.into()],
    );

    let row = BlobRow::find_by_statement(stmt).one(db).await?;
    Ok(row.map(|r| r.data))
}

pub struct Thumbnail {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mimetype: &'static str,
}

pub fn make_thumbnail(data: &[u8], mimetype: &str) -> Option<Thumbnail> {
    if !mimetype.starts_with("image/") {
        return None;
    }
    let img = image::load_from_memory(data).ok()?;
    let thumb = img.thumbnail(256, 256).to_rgb8();
    let (width, height) = thumb.dimensions();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    image::DynamicImage::ImageRgb8(thumb)
        .write_with_encoder(encoder)
        .ok()?;
    Some(Thumbnail {
        data: buf,
        width,
        height,
        mimetype: "image/jpeg",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_blob_matches_known_vectors() {
        // Canonical SHA-256 test vectors — content addressing must stay stable,
        // since these hashes are the primary key in `file_blobs`.
        assert_eq!(
            hash_blob(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_blob(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hash_blob_is_deterministic_and_content_dependent() {
        let a = hash_blob(b"hello world");
        assert_eq!(a, hash_blob(b"hello world"));
        assert_ne!(a, hash_blob(b"hello world!"));
    }

    #[test]
    fn make_thumbnail_skips_non_images() {
        assert!(make_thumbnail(b"not an image", "text/plain").is_none());
        assert!(make_thumbnail(b"", "application/pdf").is_none());
    }
}
