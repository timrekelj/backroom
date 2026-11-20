//! Image processing: sniff, validate, decode, centre-crop or fit, resize,
//! re-encode. Kinds that keep animation (emojis) turn animated GIF/WebP into a
//! resized GIF with the original frame delays.

use std::io::Cursor;
use std::sync::OnceLock;

use bytes::Bytes;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::imageops::{self, FilterType};
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, Frame, ImageDecoder, ImageEncoder, ImageFormat, Limits, RgbaImage,
};
use tokio::sync::Semaphore;

use super::{MediaKind, Shape};

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
/// Animation limits (decompression-bomb guards): canvas edge, frames, and
/// total decoded pixels over all frames. Frames are resized one at a time, so
/// peak memory is one source frame plus the small output frames.
const ANIM_MAX_DIMENSION: u32 = 4096;
const ANIM_MAX_FRAMES: usize = 200;
const ANIM_MAX_PIXELS: u64 = 100_000_000;

/// Why an image was refused. Every variant maps to INVALID_ARGUMENT except `Internal`.
#[derive(Debug, PartialEq, Eq)]
pub enum MediaError {
    TooLarge,
    /// Animation with too many frames or too many decoded pixels.
    TooComplex,
    UnsupportedType(String),
    TypeMismatch,
    Corrupt,
    DimensionsTooLarge,
    Internal(String),
}

impl MediaError {
    pub fn message(&self) -> String {
        match self {
            Self::TooLarge => "image is too large (avatars and icons: at most 8 MiB, emojis: at most 2 MiB)".to_owned(),
            Self::TooComplex => format!("animation must have at most {ANIM_MAX_FRAMES} frames and {} megapixels in total", ANIM_MAX_PIXELS / 1_000_000),
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
    /// Whether the output is an animation (GIF).
    pub animated: bool,
}

impl Processed {
    /// File extension used in the storage key.
    pub fn extension(&self) -> &'static str {
        if self.animated { "gif" } else { "webp" }
    }
}

fn permits() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(MAX_CONCURRENT))
}

/// Validates and processes `data` for `kind` on a blocking thread.
pub async fn process(kind: MediaKind, data: Vec<u8>, content_type: String) -> Result<Processed, MediaError> {
    // Cheap checks first, before queueing for a decode slot.
    if data.len() > kind.max_input_bytes() {
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

    if kind.animated()
        && let Shape::Fit(edge) = kind.shape()
        && let Some(processed) = animated(data, sniffed, edge)?
    {
        return Ok(processed);
    }

    let image = decode(data, sniffed)?;
    let resized: RgbaImage = match kind.shape() {
        Shape::CropSquare(side) => {
            let square = centre_square(image);
            imageops::resize(&square, side, side, FilterType::Lanczos3)
        }
        Shape::Fit(edge) => fit(image.into_rgba8(), edge),
    };
    encode_webp(&resized)
}

fn encode_webp(image: &RgbaImage) -> Result<Processed, MediaError> {
    // Lossless WebP: pure Rust (the `image` crate has no lossy encoder) and a
    // 256x256 image stays well under 256 KiB.
    let (w, h) = image.dimensions();
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(image.as_raw(), w, h, ExtendedColorType::Rgba8)
        .map_err(|e| MediaError::Internal(format!("encoding webp: {e}")))?;
    Ok(Processed {
        bytes: out.into(),
        content_type: "image/webp",
        animated: false,
    })
}

/// Size that fits `w`x`h` within `edge`x`edge`, aspect kept, never upscaled.
fn fit_dimensions(w: u32, h: u32, edge: u32) -> (u32, u32) {
    if w <= edge && h <= edge {
        return (w, h);
    }
    let scale = f64::min(f64::from(edge) / f64::from(w), f64::from(edge) / f64::from(h));
    let scaled = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, edge);
    (scaled(w), scaled(h))
}

fn fit(image: RgbaImage, edge: u32) -> RgbaImage {
    let (w, h) = image.dimensions();
    let (nw, nh) = fit_dimensions(w, h, edge);
    if (nw, nh) == (w, h) {
        image
    } else {
        imageops::resize(&image, nw, nh, FilterType::Lanczos3)
    }
}

/// Animated GIF / WebP -> GIF fitted within `edge`, keeping frame delays.
/// `Ok(None)` if the input is not an animation of at least two frames and the
/// caller should take the static path; a single-frame GIF is handled here as a
/// static image so it is not decoded twice.
fn animated(data: &[u8], format: ImageFormat, edge: u32) -> Result<Option<Processed>, MediaError> {
    let frames = match format {
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(data)).map_err(map_decode_error)?;
            check_canvas(decoder.dimensions())?;
            let mut limits = Limits::default();
            limits.max_alloc = Some(512 * 1024 * 1024);
            decoder.set_limits(limits).map_err(map_decode_error)?;
            collect_frames(decoder.dimensions(), decoder.into_frames(), edge)?
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(data)).map_err(map_decode_error)?;
            check_canvas(decoder.dimensions())?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            let mut limits = Limits::default();
            limits.max_alloc = Some(512 * 1024 * 1024);
            decoder.set_limits(limits).map_err(map_decode_error)?;
            collect_frames(decoder.dimensions(), decoder.into_frames(), edge)?
        }
        _ => return Ok(None),
    };
    match frames.len() {
        0 => Err(MediaError::Corrupt),
        1 => encode_webp(frames[0].buffer()).map(Some),
        _ => {
            let mut out = Vec::new();
            {
                let mut encoder = GifEncoder::new_with_speed(&mut out, 10);
                encoder
                    .set_repeat(Repeat::Infinite)
                    .map_err(|e| MediaError::Internal(format!("encoding gif: {e}")))?;
                encoder
                    .encode_frames(frames)
                    .map_err(|e| MediaError::Internal(format!("encoding gif: {e}")))?;
            }
            Ok(Some(Processed {
                bytes: out.into(),
                content_type: "image/gif",
                animated: true,
            }))
        }
    }
}

fn check_canvas((w, h): (u32, u32)) -> Result<(), MediaError> {
    if w == 0 || h == 0 || w > ANIM_MAX_DIMENSION || h > ANIM_MAX_DIMENSION {
        return Err(MediaError::DimensionsTooLarge);
    }
    Ok(())
}

/// Resizes every frame as it is decoded, enforcing the frame and pixel budgets
/// before the next frame is decoded.
fn collect_frames(
    (w, h): (u32, u32),
    frames: image::Frames<'_>,
    edge: u32,
) -> Result<Vec<Frame>, MediaError> {
    let (nw, nh) = fit_dimensions(w, h, edge);
    let canvas = u64::from(w) * u64::from(h);
    let mut out = Vec::new();
    for frame in frames {
        if out.len() >= ANIM_MAX_FRAMES || canvas * (out.len() as u64 + 1) > ANIM_MAX_PIXELS {
            return Err(MediaError::TooComplex);
        }
        let frame = frame.map_err(map_decode_error)?;
        let delay = frame.delay();
        let buffer = frame.into_buffer();
        let buffer = if (nw, nh) == buffer.dimensions() {
            buffer
        } else {
            imageops::resize(&buffer, nw, nh, FilterType::Triangle)
        };
        out.push(Frame::from_parts(buffer, 0, 0, delay));
    }
    Ok(out)
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
