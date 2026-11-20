//! Image processing: sniff, validate, decode, centre-crop, resize, re-encode.

use std::io::Cursor;
use std::sync::OnceLock;

use bytes::Bytes;
use image::codecs::webp::WebPEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, Limits, RgbaImage};
use tokio::sync::Semaphore;

use super::MediaKind;

/// Largest accepted upload.
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// Largest accepted width or height of the source image.
const MAX_DIMENSION: u32 = 8192;
/// Largest accepted pixel count; with 4 bytes/pixel this bounds decode memory
/// to ~200 MB per image.
const MAX_PIXELS: u64 = 50_000_000;
/// Decodes running at once. Decoding is CPU and memory heavy, so concurrent
/// uploads queue instead of multiplying peak memory.
const MAX_CONCURRENT: usize = 2;

/// Why an image was refused. Every variant maps to INVALID_ARGUMENT except `Internal`.
#[derive(Debug, PartialEq, Eq)]
pub enum MediaError {
    TooLarge,
    UnsupportedType(String),
    TypeMismatch,
    Corrupt,
    DimensionsTooLarge,
    Internal(String),
}

impl MediaError {
    pub fn message(&self) -> String {
        match self {
            Self::TooLarge => format!("image must be at most {} MiB", MAX_INPUT_BYTES / (1024 * 1024)),
            Self::UnsupportedType(t) => format!("unsupported content type {t:?}, expected image/png, image/jpeg, image/webp or image/gif"),
            Self::TypeMismatch => "image data does not match the declared content type".to_owned(),
            Self::Corrupt => "image data could not be decoded".to_owned(),
            Self::DimensionsTooLarge => format!("image dimensions must be at most {MAX_DIMENSION}x{MAX_DIMENSION} and {MAX_PIXELS} pixels"),
            Self::Internal(e) => e.clone(),
        }
    }
}

/// A processed image ready to store.
pub struct Processed {
    pub bytes: Bytes,
    pub content_type: &'static str,
}

fn permits() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(MAX_CONCURRENT))
}

/// Validates and processes `data` for `kind` on a blocking thread.
pub async fn process(kind: MediaKind, data: Vec<u8>, content_type: String) -> Result<Processed, MediaError> {
    // Cheap checks first, before queueing for a decode slot.
    if data.len() > MAX_INPUT_BYTES {
        return Err(MediaError::TooLarge);
    }
    let declared = declared_format(&content_type)?;
    let _permit = permits().acquire().await.map_err(|e| MediaError::Internal(e.to_string()))?;
    tokio::task::spawn_blocking(move || process_blocking(kind, &data, declared))
        .await
        .map_err(|e| MediaError::Internal(format!("image task failed: {e}")))?
}

fn declared_format(content_type: &str) -> Result<ImageFormat, MediaError> {
    match content_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Ok(ImageFormat::Png),
        "image/jpeg" => Ok(ImageFormat::Jpeg),
        "image/webp" => Ok(ImageFormat::WebP),
        "image/gif" => Ok(ImageFormat::Gif),
        _ => Err(MediaError::UnsupportedType(content_type.chars().take(64).collect())),
    }
}

fn process_blocking(kind: MediaKind, data: &[u8], declared: ImageFormat) -> Result<Processed, MediaError> {
    // Sniff the real format from the bytes; the declared type must agree.
    let sniffed = image::guess_format(data).map_err(|_| MediaError::Corrupt)?;
    if !matches!(sniffed, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif) {
        return Err(MediaError::Corrupt);
    }
    if sniffed != declared {
        return Err(MediaError::TypeMismatch);
    }

    let image = decode(data, sniffed)?;
    let _ = kind.animated(); // animation is reserved for emoji kinds
    let side = kind.size();
    let square = centre_square(image);
    let resized: RgbaImage = imageops::resize(&square, side, side, FilterType::Lanczos3);

    // Lossless WebP: pure Rust (the `image` crate has no lossy encoder) and a
    // 256x256 image stays well under 256 KiB.
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(resized.as_raw(), side, side, ExtendedColorType::Rgba8)
        .map_err(|e| MediaError::Internal(format!("encoding webp: {e}")))?;
    Ok(Processed {
        bytes: out.into(),
        content_type: "image/webp",
    })
}

/// Decodes the first frame with dimension and memory limits, honouring EXIF orientation.
fn decode(data: &[u8], format: ImageFormat) -> Result<DynamicImage, MediaError> {
    let mut reader = image::ImageReader::with_format(Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(map_decode_error)?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || w > MAX_DIMENSION || h > MAX_DIMENSION || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(MediaError::DimensionsTooLarge);
    }
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(map_decode_error)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn map_decode_error(e: image::ImageError) -> MediaError {
    match e {
        image::ImageError::Limits(_) => MediaError::DimensionsTooLarge,
        _ => MediaError::Corrupt,
    }
}

/// Crops the largest centred square.
fn centre_square(image: DynamicImage) -> RgbaImage {
    let rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    let side = w.min(h);
    imageops::crop_imm(&rgba, (w - side) / 2, (h - side) / 2, side, side).to_image()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, codecs::gif::GifEncoder, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder};

    fn sample(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]))
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out).write_image(sample(w, h).as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
        out
    }

    fn webp_out(p: &Processed) -> (u32, u32) {
        let img = image::load_from_memory_with_format(&p.bytes, ImageFormat::WebP).unwrap();
        (img.width(), img.height())
    }

    #[tokio::test]
    async fn png_is_cropped_to_256_webp() {
        let p = process(MediaKind::Avatar, png(300, 100), "image/png".into()).await.unwrap();
        assert_eq!(p.content_type, "image/webp");
        assert_eq!(webp_out(&p), (256, 256));
    }

    #[tokio::test]
    async fn jpeg_and_gif_decode() {
        let rgb = DynamicImage::ImageRgba8(sample(64, 48)).into_rgb8();
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg).write_image(rgb.as_raw(), 64, 48, ExtendedColorType::Rgb8).unwrap();
        assert_eq!(webp_out(&process(MediaKind::GuildIcon, jpeg, "image/jpeg".into()).await.unwrap()), (256, 256));

        let mut gif = Vec::new();
        GifEncoder::new(&mut gif).encode(sample(20, 20).as_raw(), 20, 20, ExtendedColorType::Rgba8).unwrap();
        assert_eq!(webp_out(&process(MediaKind::Avatar, gif, "image/gif".into()).await.unwrap()), (256, 256));
    }

    #[tokio::test]
    async fn rejects_bad_input() {
        let ok = png(8, 8);
        assert_eq!(process(MediaKind::Avatar, ok.clone(), "image/bmp".into()).await.err(), Some(MediaError::UnsupportedType("image/bmp".into())));
        assert_eq!(process(MediaKind::Avatar, ok, "image/jpeg".into()).await.err(), Some(MediaError::TypeMismatch));
        assert_eq!(process(MediaKind::Avatar, vec![1, 2, 3, 4], "image/png".into()).await.err(), Some(MediaError::Corrupt));
        let mut truncated = png(64, 64);
        truncated.truncate(truncated.len() / 2);
        assert_eq!(process(MediaKind::Avatar, truncated, "image/png".into()).await.err(), Some(MediaError::Corrupt));
        assert_eq!(process(MediaKind::Avatar, vec![0; MAX_INPUT_BYTES + 1], "image/png".into()).await.err(), Some(MediaError::TooLarge));
        assert_eq!(process(MediaKind::Avatar, png(MAX_DIMENSION + 1, 1), "image/png".into()).await.err(), Some(MediaError::DimensionsTooLarge));
    }
}
