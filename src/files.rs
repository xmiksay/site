use sha2::{Digest, Sha256};

pub fn hash_blob(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub struct Thumbnail {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mimetype: &'static str,
}

/// Upper bounds for decoding an uploaded image: a small file can still
/// declare enormous dimensions (a decompression bomb).
const MAX_DECODE_SIDE: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

/// A reader over `data` with its format sniffed from the bytes (never
/// trusted from a name or mimetype) and decode limits set.
fn reader(data: &[u8]) -> Option<image::ImageReader<std::io::Cursor<&[u8]>>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    Some(reader)
}

pub fn make_thumbnail(data: &[u8], mimetype: &str) -> Option<Thumbnail> {
    if !mimetype.starts_with("image/") {
        return None;
    }
    let img = reader(data)?.decode().ok()?;
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

/// Longest side handed to a model: Anthropic's recommended maximum — every
/// provider downscales larger images itself, so more pixels only cost
/// tokens and upload time.
pub const MODEL_IMAGE_MAX_SIDE: u32 = 1568;
/// An in-bounds image this small goes to the model byte-for-byte.
const MODEL_IMAGE_PASSTHROUGH_BYTES: usize = 1024 * 1024;

/// An image re-encoded for a model's image content block.
pub struct ModelImage {
    pub media_type: String,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The raster formats every provider adapter (Anthropic, OpenAI, Gemini)
/// accepts as an image block.
pub fn is_model_image(mimetype: &str) -> bool {
    matches!(
        mimetype,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

/// The media type a provider accepts for bytes of `format`, if any.
fn model_media_type(format: image::ImageFormat) -> Option<&'static str> {
    match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        image::ImageFormat::Gif => Some("image/gif"),
        _ => None,
    }
}

/// `data` ready for a model's eyes: `None` for anything but a decodable
/// [`is_model_image`]. Downscaled so the longest side is at most
/// [`MODEL_IMAGE_MAX_SIDE`]; re-encoded only when it had to shrink, is
/// large, or its bytes are not a format providers take — PNG when it has
/// transparency, else JPEG. The media type always comes from the bytes: a
/// mislabelled file sent as its label is rejected by the provider, and
/// that tool result would then break every later turn of the session.
pub fn image_for_model(data: &[u8], mimetype: &str) -> Option<ModelImage> {
    if !is_model_image(mimetype) {
        return None;
    }
    let sniffed = reader(data)?.format().and_then(model_media_type);
    let (width, height) = reader(data)?.into_dimensions().ok()?;
    let fits = width.max(height) <= MODEL_IMAGE_MAX_SIDE;
    if let Some(media_type) =
        sniffed.filter(|_| fits && data.len() <= MODEL_IMAGE_PASSTHROUGH_BYTES)
    {
        return Some(ModelImage {
            media_type: media_type.to_string(),
            data: data.to_vec(),
            width,
            height,
        });
    }
    let img = reader(data)?.decode().ok()?;
    let img = if fits {
        img
    } else {
        // Triangle is plenty for a downscale a model looks at, and much
        // cheaper than Lanczos on a large photo.
        img.resize(
            MODEL_IMAGE_MAX_SIDE,
            MODEL_IMAGE_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        )
    };
    let mut buf = Vec::new();
    let media_type = if img.color().has_alpha() {
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .ok()?;
        "image/png"
    } else {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
        image::DynamicImage::ImageRgb8(img.to_rgb8())
            .write_with_encoder(encoder)
            .ok()?;
        "image/jpeg"
    };
    Some(ModelImage {
        media_type: media_type.to_string(),
        data: buf,
        width: img.width(),
        height: img.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let img = if alpha {
            image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
        } else {
            image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
        };
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode test png");
        buf
    }

    #[test]
    fn image_for_model_passes_small_images_through() {
        let data = png(40, 20, false);
        let out = image_for_model(&data, "image/png").expect("an image");
        assert_eq!(out.media_type, "image/png");
        assert_eq!(out.data, data);
        assert_eq!((out.width, out.height), (40, 20));
    }

    #[test]
    fn image_for_model_downscales_to_the_max_side_keeping_the_aspect() {
        let out = image_for_model(&png(3136, 1000, false), "image/png").expect("an image");
        assert_eq!((out.width, out.height), (MODEL_IMAGE_MAX_SIDE, 500));
        assert_eq!(out.media_type, "image/jpeg");
        let decoded = image::load_from_memory(&out.data).expect("decodable");
        assert_eq!(decoded.width(), MODEL_IMAGE_MAX_SIDE);
    }

    #[test]
    fn image_for_model_keeps_transparency_as_png() {
        let out = image_for_model(&png(1000, 2000, true), "image/png").expect("an image");
        assert_eq!(out.media_type, "image/png");
        assert_eq!((out.width, out.height), (784, MODEL_IMAGE_MAX_SIDE));
    }

    fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut std::io::Cursor::new(&mut buf), format)
            .expect("encode test image");
        buf
    }

    #[test]
    fn image_for_model_takes_the_media_type_from_the_bytes() {
        let jpeg = encoded(40, 20, image::ImageFormat::Jpeg);
        let out = image_for_model(&jpeg, "image/png").expect("an image");
        assert_eq!(out.media_type, "image/jpeg");
        assert_eq!(out.data, jpeg);
    }

    #[test]
    fn image_for_model_re_encodes_bytes_providers_do_not_take() {
        let bmp = encoded(40, 20, image::ImageFormat::Bmp);
        let out = image_for_model(&bmp, "image/png").expect("an image");
        assert_eq!(out.media_type, "image/jpeg");
        assert_eq!((out.width, out.height), (40, 20));
        assert_eq!(
            image::guess_format(&out.data).expect("format"),
            image::ImageFormat::Jpeg
        );
    }

    #[test]
    fn image_for_model_skips_other_types_and_garbage() {
        assert!(image_for_model(&png(10, 10, false), "image/svg+xml").is_none());
        assert!(image_for_model(b"text", "text/plain").is_none());
        assert!(image_for_model(b"not a png", "image/png").is_none());
    }

    #[test]
    fn hash_blob_matches_known_vectors() {
        // Canonical SHA-256 test vectors — content addressing must stay stable,
        // since these hashes are the primary key in `file_blobs` and the storage keys.
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
