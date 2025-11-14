use crate::model::{Activity, Participant, Room};
use crate::service::renderer::timeline::{TimelineRenderer, TimelineRendererError};
use crate::service::renderer::view::{FillStyle, RenderSection, StreamingSection, Timeline, TimelineEntry};
use image::imageops::FilterType;
use image::{imageops, ImageFormat, ImageReader};
use kmeans_colors::{get_kmeans, Kmeans, Sort};
use moka::future::Cache;
use palette::cast::from_component_slice;
use palette::{FromColor, IntoColor, Lab, Srgba};
use reqwest::Client;
use serenity::all::{ChannelId, CreateAttachment, CreateMessage, Http, MessageFlags, Timestamp, UserId};
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use tiny_skia::{Color, Pixmap};
use tokio::time::Instant;
use tracing::error;

#[derive(Debug)]
pub enum ReportServiceError{
    GenericError(String),
    RenderingError(TimelineRendererError),
}

impl From<TimelineRendererError> for ReportServiceError {
    fn from(err: TimelineRendererError) -> Self {
        ReportServiceError::RenderingError(err)
    }
}

pub type ReportServiceResult<T> = Result<T, ReportServiceError>;

#[derive(Clone)]
struct EntryVisual {
    pub avatar: Pixmap,
    pub active_color: Color,
    pub inactive_color: Color,
}

pub struct ReportService {
    client: Client,
    cache: Cache<UserId, EntryVisual>,
    renderer: Arc<TimelineRenderer>,
    report_channel_id: ChannelId,
}

#[derive(Debug, Clone)]
pub struct RoomDTO {
    pub created_at: Instant,
    pub timestamp: Timestamp,
    pub channel_id: ChannelId,
    pub participants: Vec<Participant>,
}

impl RoomDTO {
    pub fn from_room(room: &Room) -> Self {
        let participants = room.participants().iter().map(|p| {
            p.clone()
        }).collect();

        RoomDTO {
            created_at: room.created_at(),
            timestamp: room.timestamp(),
            channel_id: room.channel_id(),
            participants,
        }
    }
}

impl ReportService {
    pub fn new(client: Client, report_channel_id: ChannelId) -> Self {
        Self{
            client,
            cache: Cache::new(100),
            renderer: Arc::new(TimelineRenderer::new()),
            report_channel_id,
        }
    }

    async fn create_timeline(&self, now: Instant, room: &RoomDTO, avatar_size: u32) -> ReportServiceResult<Timeline> {
        let mut entries = Vec::new();
        for participant in &room.participants {
            let entry = self.cache.entry(participant.user_id()).or_try_insert_with::<_, String>(async {
                let request = match self.client.get(participant.face()).build() {
                    Ok(request) => request,
                    Err(err) => {
                        error!("CRITICAL: failed to build request, use fallback avatar: {:?}", err);
                        self.client.get("https://cdn.discordapp.com/embed/avatars/0.png").build().expect("failed to build request for default avatar!")
                    }
                };

                let response = match self.client.execute(request).await {
                    Ok(response) => response,
                    Err(err) => return Err(err.to_string())
                };

                let avatar_bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(err) => return Err(err.to_string())
                };

                let task = tokio::task::spawn_blocking(move || {
                    let avatar_image = match ImageReader::new(BufReader::new(Cursor::new(avatar_bytes))).with_guessed_format() {
                        Ok(image) => match image.decode() {
                            Ok(image) => image,
                            Err(err) => return Err(err.to_string())
                        },
                        Err(err) => {
                            return Err(err.to_string())
                        }
                    };
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
                    match avatar_image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png) {
                        Ok(_) => {},
                        Err(err) => {
                            return Err(err.to_string())
                        }
                    };

                    let inactive_color = Color::from_rgba(active_color.red(), active_color.green(), active_color.blue(), active_color.alpha()*0.35).unwrap();

                    match Pixmap::decode_png(&bytes) {
                        Ok(pixmap) => Ok(EntryVisual {
                            avatar: pixmap,
                            active_color,
                            inactive_color
                        }),
                        Err(err) => Err(err.to_string())
                    }
                });

                let pixmap = match task.await {
                    Ok(pixmap) => pixmap,
                    Err(err) => return Err(err.to_string())
                };

                pixmap
            }).await;

            let visual = match entry {
                Ok(entry) => entry.into_value(),
                Err(err) => return Err(ReportServiceError::GenericError(err.to_string())),
            };

            entries.push(TimelineEntry{
                avatar: visual.avatar,
                sections: convert_to_render_sections(room.created_at, now, participant.history()),
                streaming_sections: convert_to_streaming_sections(room.created_at, now, participant.history()),
                active_color: visual.active_color,
                inactive_color: visual.inactive_color,
            });
        }

        let timeline = Timeline{
            start: room.created_at,
            end: now,
            indicator: None,
            entries
        };

        Ok(timeline)
    }

    pub async fn send_room_report(&self, http: &Http, now: Instant, room: &RoomDTO) -> ReportServiceResult<()> {
        let timeline = self.create_timeline(now, room, 64).await?;

        let renderer = self.renderer.clone();

        let task = tokio::task::spawn_blocking(move || {
            let pixmap = renderer.generate_image(&timeline)?;
            let encoded_image = match pixmap.encode_png() {
                Ok(b) => b,
                Err(err) => {
                    return Err(ReportServiceError::GenericError(err.to_string()))
                }
            };
            Ok(encoded_image)
        });

        let encoded_image = match task.await {
            Ok(image) => image?,
            Err(err) => return Err(ReportServiceError::GenericError(err.to_string()))
        };


        match self.report_channel_id
            .send_message(
                http,
                CreateMessage::new()
                    .embed(self.renderer.generate_ongoing_embed(
                        now,
                        Timestamp::now(),
                        room,
                    ))
                    .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                    .add_file(CreateAttachment::bytes(encoded_image, "thumbnail.png")),
            )
            .await {
            Ok(_) => Ok(()),
            Err(err) => Err(ReportServiceError::GenericError(err.to_string())),
        }
    }
}

fn convert_to_render_sections(start: Instant, end: Instant, history: &Vec<Activity>) -> Vec<RenderSection> {
    let duration_sec = (end - start).as_secs_f32();
    let mut render_sections = Vec::new();

    for i in 0..history.len() {
        let current = &history[i];
        let fill_style = FillStyle::from_flags(current.flags());

        let start_ratio = (current.start() - start).as_secs_f32()/duration_sec;
        let end_ratio = (current.end().unwrap_or(end) - start).as_secs_f32()/duration_sec;

        render_sections.push(RenderSection {
            start_ratio,
            end_ratio,
            fill_style,
        })
    }

    render_sections
}

fn convert_to_streaming_sections(start: Instant, end: Instant, history: &Vec<Activity>) -> Vec<StreamingSection> {
    let duration_sec = (end - start).as_secs_f32();
    let mut streaming_sections = Vec::new();

    // Always keep streaming start activity.
    let mut streaming_start_activity: Option<&Activity> = None;

    for i in 0..history.len() {
        let current_activity = &history[i];

        // update current streaming start
        match streaming_start_activity {
            Some(streaming_start) => {
                if !current_activity.flags().is_sharing_screen {
                    let start_ratio = (streaming_start.start() - start).as_secs_f32()/duration_sec;
                    let end_ratio = (current_activity.start() - start).as_secs_f32()/duration_sec;

                    streaming_sections.push(StreamingSection{
                        start_ratio,
                        end_ratio,
                    });

                    streaming_start_activity = None;
                }
            },
            None => {
                if current_activity.flags().is_sharing_screen {
                    streaming_start_activity = Some(&history[i]);
                }
            }
        }

        // detect disconnection from voice channel
        if let Some(streaming_start) = &streaming_start_activity {
            let terminated = if i == history.len() - 1 {
                true
            } else {
                !history[i+1].is_following(current_activity)
            };

            if terminated {
                let start_ratio = (streaming_start.start() - start).as_secs_f32()/duration_sec;
                let end_ratio = (current_activity.end().unwrap_or(end) - start).as_secs_f32()/duration_sec;

                streaming_sections.push(StreamingSection{
                    start_ratio,
                    end_ratio,
                });
                streaming_start_activity = None;
            }
        }
    }

    streaming_sections
}