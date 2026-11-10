use image::imageops::FilterType;
use image::{ImageReader, imageops};
use kmeans_colors::{Kmeans, Sort, get_kmeans};
use moka::future::Cache;
use palette::cast::from_component_slice;
use palette::{FromColor, IntoColor, Lab, Srgba};
use serenity::all::{GuildId, Http, UserId};
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use thiserror::Error;
use tiny_skia::{Color, IntSize, Pixmap};

/// Avatar size (pixels) used for color extraction and rendering.
const AVATAR_SIZE: u32 = 64;
/// Maximum number of cached member visuals.
const CACHE_CAPACITY: u64 = 128;
/// Lab lightness range considered readable (excludes near-black and near-white).
const MIN_LIGHTNESS: f32 = 20.0;
const MAX_LIGHTNESS: f32 = 90.0;
/// k-means clustering parameters.
const KMEANS_CLUSTERS: usize = 3;
const KMEANS_ITERATIONS: usize = 30;
const KMEANS_CONVERGENCE: f32 = 1.0;
const KMEANS_RUNS: usize = 5;
/// Alpha factor applied to active color for inactive/muted state.
const INACTIVE_ALPHA_FACTOR: f32 = 0.35;
/// Lightness multiplier applied to active color for streaming state.
const STREAMING_LIGHTNESS_FACTOR: f32 = 0.4;
/// Maximum avatar download size (10 MiB). Discord enforces server-side limits,
/// but a compromised or misbehaving host could serve an arbitrarily large body.
const MAX_AVATAR_BYTES: usize = 10 * 1024 * 1024;

/// Visual assets and color palette derived from a member's avatar.
#[derive(Clone)]
pub struct MemberVisual {
    /// The processed and resized avatar image.
    pub avatar: Pixmap,
    /// The dominant color of the avatar, used for active participation states.
    pub active_color: Color,
    /// An alpha-faded version of the active color (RGB unchanged) for muted/inactive states.
    pub inactive_color: Color,
    /// A distinct color derived from the avatar for streaming/screen sharing states.
    pub streaming_color: Color,
}

impl MemberVisual {
    /// Builds a fallback visual for a member whose avatar could not be fetched.
    ///
    /// The active color is chosen deterministically from a palette of colors
    /// whose Lab lightness stays within the same (20, 90) range used by the
    /// real extraction, so the fallback follows the same color rules.
    pub fn fallback(user_id: UserId) -> Self {
        const PALETTE: [(u8, u8, u8); 8] = [
            (200, 50, 50),
            (50, 180, 50),
            (50, 50, 200),
            (220, 140, 30),
            (140, 50, 200),
            (30, 180, 180),
            (220, 80, 150),
            (220, 200, 50),
        ];

        let (r, g, b) = PALETTE[(user_id.get() as usize) % PALETTE.len()];
        let active_color = Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
            .unwrap_or(Color::BLACK);

        let inactive_color = Color::from_rgba(
            active_color.red().clamp(0.0, 1.0),
            active_color.green().clamp(0.0, 1.0),
            active_color.blue().clamp(0.0, 1.0),
            (active_color.alpha() * INACTIVE_ALPHA_FACTOR).clamp(0.0, 1.0),
        )
        .unwrap_or(Color::BLACK);

        let streaming_color = {
            let mut lab_color: Lab = Srgba::new(
                active_color.red(),
                active_color.green(),
                active_color.blue(),
                active_color.alpha(),
            )
            .into_color();
            lab_color.l *= STREAMING_LIGHTNESS_FACTOR;
            let rgba_color = Srgba::from_color(lab_color);
            Color::from_rgba(
                rgba_color.red.clamp(0.0, 1.0),
                rgba_color.green.clamp(0.0, 1.0),
                rgba_color.blue.clamp(0.0, 1.0),
                rgba_color.alpha.clamp(0.0, 1.0),
            )
            .unwrap_or(Color::BLACK)
        };

        let mut avatar = Pixmap::new(1, 1).unwrap();
        avatar.fill(active_color);

        Self {
            avatar,
            active_color,
            inactive_color,
            streaming_color,
        }
    }
}

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("Serenity request failed: {0}")]
    Serenity(#[from] serenity::Error),

    #[error("Network request failed: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Image processing failed: {0}")]
    Image(#[from] image::ImageError),

    #[error("Pixmap construction error")]
    PixmapConstruction,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Async task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Response body too large (limit: {0} bytes)")]
    TooLarge(usize),
}

/// Checks that a response body does not exceed the avatar size limit.
///
/// Validates both the `Content-Length` header (if present) and the actual
/// body length, so chunked transfers without a header are still capped.
pub(crate) fn check_avatar_size(content_length: Option<u64>, body_len: usize) -> Result<(), AssetError> {
    if let Some(len) = content_length
        && len as usize > MAX_AVATAR_BYTES
    {
        return Err(AssetError::TooLarge(MAX_AVATAR_BYTES));
    }
    if body_len > MAX_AVATAR_BYTES {
        return Err(AssetError::TooLarge(MAX_AVATAR_BYTES));
    }
    Ok(())
}

/// Extracts the dominant color from a resized avatar image using k-means
/// clustering in Lab color space.
///
/// Returns `None` if all pixels are filtered out by the lightness guard
/// (near-black or near-white avatars), so the caller can treat this as the
/// "empty lab" special case (all colors BLACK).
pub(crate) fn extract_active_color(
    buffer: &image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
) -> Option<Color> {
    let lab: Vec<Lab> = from_component_slice::<Srgba<u8>>(buffer)
        .iter()
        .map(|x| x.color.into_linear().into_color())
        .filter(|x: &Lab| MIN_LIGHTNESS < x.l && x.l < MAX_LIGHTNESS)
        .collect();

    if lab.is_empty() {
        return None;
    }

    let mut result = Kmeans::new();
    for i in 0..KMEANS_RUNS {
        let run_result = get_kmeans(
            KMEANS_CLUSTERS,
            KMEANS_ITERATIONS,
            KMEANS_CONVERGENCE,
            false,
            &lab,
            i as u64,
        );
        if run_result.score < result.score {
            result = run_result;
        }
    }

    let res = Lab::sort_indexed_colors(&result.centroids, &result.indices);
    let dominant_color = Lab::get_dominant_color(&res);

    match dominant_color {
        Some(color) => {
            let color = Srgba::from_color(color);
            Some(Color::from_rgba(
                color.red.clamp(0.0, 1.0),
                color.green.clamp(0.0, 1.0),
                color.blue.clamp(0.0, 1.0),
                color.alpha.clamp(0.0, 1.0),
            )
            .unwrap_or(Color::BLACK))
        }
        None => Some(Color::BLACK),
    }
}

/// Derives the inactive (muted) color from the active color.
pub(crate) fn derive_inactive_color(active_color: Color) -> Color {
    Color::from_rgba(
        active_color.red().clamp(0.0, 1.0),
        active_color.green().clamp(0.0, 1.0),
        active_color.blue().clamp(0.0, 1.0),
        (active_color.alpha() * INACTIVE_ALPHA_FACTOR).clamp(0.0, 1.0),
    )
    .unwrap_or(Color::BLACK)
}

/// Derives the streaming color from the active color by darkening in Lab space.
pub(crate) fn derive_streaming_color(active_color: Color) -> Color {
    let mut lab_color: Lab = Srgba::new(
        active_color.red(),
        active_color.green(),
        active_color.blue(),
        active_color.alpha(),
    )
    .into_color();
    lab_color.l *= STREAMING_LIGHTNESS_FACTOR;
    let rgba_color = Srgba::from_color(lab_color);
    Color::from_rgba(
        rgba_color.red.clamp(0.0, 1.0),
        rgba_color.green.clamp(0.0, 1.0),
        rgba_color.blue.clamp(0.0, 1.0),
        rgba_color.alpha.clamp(0.0, 1.0),
    )
    .unwrap_or(Color::BLACK)
}

/// Decodes raw avatar bytes into a `DynamicImage`.
///
/// Rejects invalid input (decode failure).
pub(crate) fn decode_avatar(avatar_bytes: Vec<u8>) -> Result<image::DynamicImage, AssetError> {
    let image_reader = ImageReader::new(BufReader::new(Cursor::new(avatar_bytes)))
        .with_guessed_format()?;
    let decoded = image_reader.decode()?;
    Ok(decoded)
}

/// Renders a decoded avatar image into a `MemberVisual`.
///
/// Processing (resize, color extract, pixmap). Only PixmapConstruction can fail.
pub(crate) fn render_avatar(
    decoded: image::DynamicImage,
    avatar_size: u32,
) -> Result<MemberVisual, AssetError> {
    let resized = imageops::resize(&decoded, avatar_size, avatar_size, FilterType::Lanczos3);

    let (active_color, inactive_color, streaming_color) = match extract_active_color(&resized) {
        None => {
            // Empty lab: all colors are BLACK (matches original early-return).
            (Color::BLACK, Color::BLACK, Color::BLACK)
        }
        Some(active_color) => {
            let inactive_color = derive_inactive_color(active_color);
            let streaming_color = derive_streaming_color(active_color);
            (active_color, inactive_color, streaming_color)
        }
    };

    let pixmap = Pixmap::from_vec(
        resized.into_raw(),
        IntSize::from_wh(avatar_size, avatar_size).unwrap(),
    )
    .ok_or(AssetError::PixmapConstruction)?;

    Ok(MemberVisual {
        avatar: pixmap,
        active_color,
        inactive_color,
        streaming_color,
    })
}

/// Processes raw avatar bytes into a `MemberVisual`.
///
/// Handles image decoding, resizing, color extraction, and pixmap construction.
/// This is a pure-ish function (no I/O) that can be called from `spawn_blocking`.
pub(crate) fn process_avatar(avatar_bytes: Vec<u8>, avatar_size: u32) -> Result<MemberVisual, AssetError> {
    let decoded = decode_avatar(avatar_bytes)?;
    render_avatar(decoded, avatar_size)
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn check_avatar_size_rejects_large_content_length() {
        assert!(check_avatar_size(Some(11 * 1024 * 1024), 0).is_err());
    }

    #[test]
    fn check_avatar_size_rejects_large_body() {
        assert!(check_avatar_size(None, 11 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_avatar_size_accepts_normal() {
        assert!(check_avatar_size(Some(1024), 1024).is_ok());
    }

    #[test]
    fn check_avatar_size_accepts_at_cap() {
        assert!(check_avatar_size(Some(MAX_AVATAR_BYTES as u64), MAX_AVATAR_BYTES).is_ok());
    }

    #[test]
    fn derive_inactive_color_preserves_rgb() {
        let active = Color::from_rgba(0.5, 0.3, 0.8, 1.0).unwrap();
        let inactive = derive_inactive_color(active);
        assert!((inactive.red() - 0.5).abs() < 0.01);
        assert!((inactive.green() - 0.3).abs() < 0.01);
        assert!((inactive.blue() - 0.8).abs() < 0.01);
        assert!((inactive.alpha() - 0.35).abs() < 0.01);
    }

    #[test]
    fn derive_streaming_color_darkens() {
        let active = Color::from_rgba(0.5, 0.3, 0.8, 1.0).unwrap();
        let streaming = derive_streaming_color(active);
        // Streaming color should be darker (lower lightness)
        // Just verify it's not the same as active
        assert!(streaming != active);
    }

    #[test]
    fn fallback_produces_valid_colors() {
        let visual = MemberVisual::fallback(UserId::new(42));
        // All colors should be valid (not NaN)
        assert!(visual.active_color.red().is_finite());
        assert!(visual.inactive_color.red().is_finite());
        assert!(visual.streaming_color.red().is_finite());
    }

    #[test]
    fn fallback_is_deterministic() {
        let v1 = MemberVisual::fallback(UserId::new(42));
        let v2 = MemberVisual::fallback(UserId::new(42));
        assert_eq!(v1.active_color, v2.active_color);
    }

    #[test]
    fn decode_avatar_rejects_garbage() {
        assert!(decode_avatar(vec![0, 1, 2, 3, 4]).is_err());
    }

    #[test]
    fn decode_avatar_accepts_png() {
        // create red image
        let mut buf = image::ImageBuffer::new(4, 4);
        for p in buf.pixels_mut() {
            *p = image::Rgba([255, 0, 0, 255]);
        }
        let img = image::DynamicImage::ImageRgba8(buf);
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let bytes = cursor.into_inner();
        assert!(decode_avatar(bytes).is_ok());
    }

    #[test]
    fn render_avatar_produces_correct_size() {
        let buf = image::ImageBuffer::new(4, 4);
        let decoded = image::DynamicImage::ImageRgba8(buf);
        let visual = render_avatar(decoded, 2).unwrap();
        assert_eq!(visual.avatar.width(), 2);
        assert_eq!(visual.avatar.height(), 2);
    }
}

/// A provider for fetching and processing member avatars into visual palettes.
///
/// It uses an asynchronous cache to avoid redundant network requests and expensive
/// image processing (k-means clustering).
#[derive(Clone)]
pub struct AssetProvider {
    client: reqwest::Client,
    serenity: Arc<Http>,
    cache: Cache<(GuildId, UserId), MemberVisual>,
    avatar_size: u32,
}

impl AssetProvider {
    pub fn new(client: reqwest::Client, serenity: Arc<Http>) -> Self {
        Self {
            client,
            serenity,
            cache: Cache::new(CACHE_CAPACITY),
            avatar_size: AVATAR_SIZE,
        }
    }

    /// Fetches a member's avatar and extracts a personalized color palette.
    pub async fn get_member_visual(
        &self,
        guild_id: GuildId,
        user_id: UserId,
    ) -> Result<MemberVisual, Arc<AssetError>> {
        let entry = self
            .cache
            .entry((guild_id, user_id))
            .or_try_insert_with::<_, AssetError>(async {
                let avatar_url = self.serenity.get_member(guild_id, user_id).await?.face();
                let request = self
                    .client
                    .get(&avatar_url)
                    .timeout(std::time::Duration::from_secs(10))
                    .build()?;

                let mut response = self.client.execute(request).await?;

                check_avatar_size(response.content_length(), 0)?;
                let mut avatar_bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    if avatar_bytes.len() + chunk.len() > MAX_AVATAR_BYTES {
                        return Err(AssetError::TooLarge(MAX_AVATAR_BYTES));
                    }
                    avatar_bytes.extend_from_slice(&chunk);
                }

                let avatar_size = self.avatar_size;

                let avatar_bytes = avatar_bytes.to_vec();
                let task = tokio::task::spawn_blocking(move || {
                    process_avatar(avatar_bytes, avatar_size)
                });

                task.await?
            })
            .await;

        Ok(entry?.into_value())
    }
}