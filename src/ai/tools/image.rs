//! Image tool results (#132): `file_read` and `design_read` show the model
//! an image as an image content block — downscaled by
//! `crate::files::image_for_model` — instead of base64 text it cannot see.
//! Every provider adapter serializes an image tool result (Anthropic inside
//! `tool_result`, OpenAI as a follow-up user message, Gemini as
//! `inlineData`), but a text-only model would reject the whole turn, so a
//! model flagged `supports_images = false` gets a text note instead.

use base64::Engine;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};

use super::common::ok_json;
use crate::ai::engine::root_session_of;
use crate::entity::{assistant_session, llm_model};
use crate::files::image_for_model;

/// Whether the model behind `session` takes image blocks: the model of the
/// session's own row, else of its root's. Unknown means yes — the column's
/// default — and a lookup failure is only logged, since it must not cost
/// the model the file's metadata.
pub(super) async fn session_sees_images(db: &DatabaseConnection, session: &SessionId) -> bool {
    match lookup_sees_images(db, session).await {
        Ok(sees) => sees.unwrap_or(true),
        Err(e) => {
            tracing::warn!(session = %session.0, "looking up the session's model: {e}");
            true
        }
    }
}

async fn lookup_sees_images(
    db: &DatabaseConnection,
    session: &SessionId,
) -> Result<Option<bool>, sea_orm::DbErr> {
    let root = root_session_of(session);
    let rows = assistant_session::Entity::find()
        .filter(assistant_session::Column::EngineSessionId.is_in([&session.0, &root.0]))
        .all(db)
        .await?;
    let row = rows
        .iter()
        .find(|r| r.engine_session_id.as_deref() == Some(session.0.as_str()))
        .or_else(|| rows.first());
    let Some(model_id) = row.and_then(|r| r.model_id) else {
        return Ok(None);
    };
    Ok(llm_model::Entity::find_by_id(model_id)
        .one(db)
        .await?
        .map(|m| m.supports_images))
}

/// The tool result for an image file: `meta` as JSON text, then the image
/// block. When the model cannot see, or the bytes do not decode, `meta`
/// carries `content: null` and a `content_error` note instead.
pub(super) async fn image_result(
    mut meta: Value,
    data: bytes::Bytes,
    mimetype: String,
    sees: bool,
) -> Vec<ContentPart> {
    if !sees {
        meta["content"] = Value::Null;
        meta["content_error"] = json!("this model cannot view images");
        return ok_json(meta);
    }
    // Decoding and resizing a large photo is CPU-heavy; keep it off the
    // async workers.
    let image = tokio::task::spawn_blocking(move || image_for_model(&data, &mimetype))
        .await
        .ok()
        .flatten();
    let Some(image) = image else {
        meta["content"] = Value::Null;
        meta["content_error"] = json!("the image could not be decoded");
        return ok_json(meta);
    };
    meta["shown"] = json!({
        "mimetype": image.media_type,
        "width": image.width,
        "height": image.height,
    });
    let mut parts = ok_json(meta);
    parts.push(ContentPart::image(
        image.media_type,
        base64::engine::general_purpose::STANDARD.encode(&image.data),
    ));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use entanglement_provider::ImageSource;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode test png");
        buf
    }

    #[tokio::test]
    async fn image_result_appends_a_downscaled_image_block() {
        let parts = image_result(
            json!({ "path": "a.png" }),
            png(2000, 100).into(),
            "image/png".into(),
            true,
        )
        .await;
        assert_eq!(parts.len(), 2);
        let meta: Value = serde_json::from_str(parts[0].as_text().expect("text")).expect("json");
        assert_eq!(meta["path"], "a.png");
        assert_eq!(meta["shown"]["width"], 1568);
        let ContentPart::Image {
            source: ImageSource::Base64 { media_type, data },
        } = &parts[1]
        else {
            panic!("expected an image part, got {:?}", parts[1]);
        };
        assert_eq!(media_type, "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("base64");
        assert_eq!(
            image::load_from_memory(&bytes).expect("decodable").width(),
            1568
        );
    }

    #[tokio::test]
    async fn image_result_falls_back_to_text_for_a_blind_model() {
        let parts = image_result(
            json!({ "path": "a.png" }),
            png(10, 10).into(),
            "image/png".into(),
            false,
        )
        .await;
        assert_eq!(parts.len(), 1);
        let meta: Value = serde_json::from_str(parts[0].as_text().expect("text")).expect("json");
        assert_eq!(meta["content"], Value::Null);
        assert_eq!(meta["content_error"], "this model cannot view images");
    }

    #[tokio::test]
    async fn image_result_reports_undecodable_bytes() {
        let parts = image_result(
            json!({}),
            bytes::Bytes::from_static(b"nope"),
            "image/png".into(),
            true,
        )
        .await;
        assert_eq!(parts.len(), 1);
        assert!(
            parts[0]
                .as_text()
                .expect("text")
                .contains("could not be decoded")
        );
    }
}
