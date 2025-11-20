use image::imageops::FilterType;
use image::{imageops, ImageFormat, ImageReader};
use kmeans_colors::{get_kmeans, Kmeans, Sort};
use moka::future::Cache;
use palette::cast::from_component_slice;
use palette::{FromColor, IntoColor, Lab, Srgba};
use serenity::all::{GuildId, UserId};
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use tiny_skia::{Color, Pixmap};

#[derive(Clone)]
pub struct MemberVisual {
    pub avatar: Pixmap,
    pub active_color: Color,
    pub inactive_color: Color,
    pub streaming_color: Color,
}

#[derive(Debug)]
pub enum AssetError{
    ReqwestError(reqwest::Error),
    ImageError(image::ImageError),
    IoError(std::io::Error),
    DecodingError,
    JoinError(tokio::task::JoinError),
}

impl From<reqwest::Error> for AssetError {
    fn from(err: reqwest::Error) -> Self{
        AssetError::ReqwestError(err)
    }
}

impl From<image::ImageError> for AssetError {
    fn from(err: image::ImageError) -> Self{
        AssetError::ImageError(err)
    }
}

impl From<std::io::Error> for AssetError {
    fn from(err: std::io::Error) -> Self{
        AssetError::IoError(err)
    }
}

impl From<tokio::task::JoinError> for AssetError {
    fn from(err: tokio::task::JoinError) -> Self{
        AssetError::JoinError(err)
    }
}

pub struct AssetService {
    client: reqwest::Client,
    cache: Cache<(GuildId, UserId), MemberVisual>,
    avatar_size: u32,
}

impl AssetService {
    pub fn new(client: reqwest::Client) -> Self {
        Self{
            client,
            cache: Cache::new(128),
            avatar_size: 64,
        }
    }

    pub async fn get_members_visual(&self, guild_id: GuildId, user_id: UserId, avatar_url: &str) -> Result<MemberVisual, Arc<AssetError>> {
        let entry = self.cache.entry((guild_id, user_id)).or_try_insert_with::<_, AssetError>(async {
            let request = self.client.get(avatar_url).build()?;

            let response = self.client.execute(request).await?;

            let avatar_bytes = response.bytes().await?;

            let avatar_size = self.avatar_size;

            let task = tokio::task::spawn_blocking(move || {
                let image_reader = ImageReader::new(BufReader::new(Cursor::new(avatar_bytes))).with_guessed_format()?;
                let avatar_image = image_reader.decode()?;
                let avatar_image = imageops::resize(&avatar_image, avatar_size, avatar_size, FilterType::Lanczos3);

                let active_color = {
                    let lab: Vec<Lab> = from_component_slice::<Srgba<u8>>(&avatar_image.to_vec())
                        .iter()
                        .map(|x| x.color.into_linear().into_color())
                        .filter(|x: &Lab| 20.0 < x.l && x.l < 90.0)
                        .collect();

                    let mut result = Kmeans::new();
                    for i in 0..5 {
                        let run_result = get_kmeans(
                            3,
                            30,
                            1.0,
                            false,
                            &lab,
                            i,
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
                            Color::from_rgba(color.red, color.green, color.blue, color.alpha).unwrap()
                        },
                        None => Color::BLACK,
                    }
                };

                let mut bytes: Vec<u8> = Vec::new();
                avatar_image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;

                let inactive_color = Color::from_rgba(active_color.red(), active_color.green(), active_color.blue(), active_color.alpha()*0.35).unwrap();
                let streaming_color = {
                    let mut lab_color: Lab = Srgba::new(active_color.red(), active_color.green(), active_color.blue(), active_color.alpha()).into_color();
                    lab_color.l = lab_color.l * 0.4;
                    let rgba_color = Srgba::from_color(lab_color);
                    Color::from_rgba(rgba_color.red, rgba_color.green, rgba_color.blue, rgba_color.alpha).unwrap()
                };

                let pixmap = match Pixmap::decode_png(&bytes){
                    Ok(pixmap) => pixmap,
                    Err(_) => return Err(AssetError::DecodingError),
                };

                Ok(MemberVisual {
                    avatar: pixmap,
                    active_color,
                    inactive_color,
                    streaming_color,
                })
            });

            let pixmap = task.await?;

            pixmap
        }).await;

        Ok(entry?.into_value())
    }
}