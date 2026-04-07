use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, imageops};
use kmeans_colors::{Kmeans, Sort, get_kmeans};
use moka::future::Cache;
use palette::cast::from_component_slice;
use palette::{FromColor, IntoColor, Lab, Srgba};
use serenity::all::{GuildId, Http, UserId};
use std::error::Error;
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use thiserror::Error;
use tiny_skia::{Color, Pixmap};

/// Visual assets and color palette derived from a member's avatar.
#[derive(Clone)]
pub struct MemberVisual {
    /// The processed and resized avatar image.
    pub avatar: Pixmap,
    /// The dominant color of the avatar, used for active participation states.
    pub active_color: Color,
    /// A desaturated/faded version of the active color for muted/inactive states.
    pub inactive_color: Color,
    /// A distinct color derived from the avatar for streaming/screen sharing states.
    pub streaming_color: Color,
}

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("Serenity request failed: {0}")]
    Serenity(#[from] serenity::Error),

    #[error("Network request failed: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Image processing failed: {0}")]
    Image(#[from] image::ImageError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to decode image: {0}")]
    PngDecoding(Box<dyn Error + Send + Sync + 'static>),

    #[error("Async task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
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
            cache: Cache::new(128),
            avatar_size: 64,
        }
    }

    /// Fetches a member's avatar and extracts a personalized color palette.
    ///
    /// The color extraction process involves:
    /// 1. Resizing the avatar to a small uniform size.
    /// 2. Converting pixels to **Lab color space** for perceptually accurate analysis.
    /// 3. Filtering out extreme lightness/darkness to ensure readable colors.
    /// 4. Running **k-means clustering** (multiple runs to find the best fit)
    ///    to identify the most representative color.
    /// 5. Deriving secondary colors (inactive/streaming) using color space transformations.
    pub async fn get_members_visual(
        &self,
        guild_id: GuildId,
        user_id: UserId,
    ) -> Result<MemberVisual, Arc<AssetError>> {
        let entry = self
            .cache
            .entry((guild_id, user_id))
            .or_try_insert_with::<_, AssetError>(async {
                let avatar_url = self.serenity.get_member(guild_id, user_id).await?.face();
                let request = self.client.get(avatar_url).build()?;

                let response = self.client.execute(request).await?;

                let avatar_bytes = response.bytes().await?;

                let avatar_size = self.avatar_size;

                let task = tokio::task::spawn_blocking(move || {
                    let image_reader = ImageReader::new(BufReader::new(Cursor::new(avatar_bytes)))
                        .with_guessed_format()?;
                    let avatar_image = image_reader.decode()?;
                    let avatar_image = imageops::resize(
                        &avatar_image,
                        avatar_size,
                        avatar_size,
                        FilterType::Lanczos3,
                    );

                    let active_color = {
                        let lab: Vec<Lab> = from_component_slice::<Srgba<u8>>(&avatar_image)
                            .iter()
                            .map(|x| x.color.into_linear().into_color())
                            .filter(|x: &Lab| 20.0 < x.l && x.l < 90.0)
                            .collect();

                        let mut result = Kmeans::new();
                        for i in 0..5 {
                            let run_result = get_kmeans(3, 30, 1.0, false, &lab, i);
                            if run_result.score < result.score {
                                result = run_result;
                            }
                        }

                        let res = Lab::sort_indexed_colors(&result.centroids, &result.indices);

                        let dominant_color = Lab::get_dominant_color(&res);

                        match dominant_color {
                            Some(color) => {
                                let color = Srgba::from_color(color);
                                Color::from_rgba(color.red, color.green, color.blue, color.alpha)
                                    .unwrap()
                            }
                            None => Color::BLACK,
                        }
                    };

                    let mut bytes: Vec<u8> = Vec::new();
                    avatar_image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;

                    let inactive_color = Color::from_rgba(
                        active_color.red(),
                        active_color.green(),
                        active_color.blue(),
                        active_color.alpha() * 0.35,
                    )
                    .unwrap();
                    let streaming_color = {
                        let mut lab_color: Lab = Srgba::new(
                            active_color.red(),
                            active_color.green(),
                            active_color.blue(),
                            active_color.alpha(),
                        )
                        .into_color();
                        lab_color.l *= 0.4;
                        let rgba_color = Srgba::from_color(lab_color);
                        Color::from_rgba(
                            rgba_color.red,
                            rgba_color.green,
                            rgba_color.blue,
                            rgba_color.alpha,
                        )
                        .unwrap()
                    };

                    let pixmap = Pixmap::decode_png(&bytes)
                        .map_err(|e| AssetError::PngDecoding(Box::new(e)))?;

                    Ok(MemberVisual {
                        avatar: pixmap,
                        active_color,
                        inactive_color,
                        streaming_color,
                    })
                });

                task.await?
            })
            .await;

        Ok(entry?.into_value())
    }
}
