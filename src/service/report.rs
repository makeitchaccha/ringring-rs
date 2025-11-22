use crate::model::{Participant, Room};
use crate::service::asset::{AssetError, AssetService};
use crate::service::renderer::timeline::{TimelineRenderer, TimelineRendererError};
use crate::service::renderer::transformer::transform;
use crate::service::renderer::view::Timeline;
use crate::service::tracker::Tracker;
use serenity::all::{ChannelId, CreateAttachment, CreateMessage, EditAttachments, EditMessage, GuildId, Http, MessageFlags, Timestamp};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Debug)]
pub enum ReportServiceError{
    GenericError(String),
    RenderingError(TimelineRendererError),
    AssetError(Arc<AssetError>),
}

impl From<TimelineRendererError> for ReportServiceError {
    fn from(err: TimelineRendererError) -> Self {
        ReportServiceError::RenderingError(err)
    }
}

pub type ReportServiceResult<T> = Result<T, ReportServiceError>;



pub struct ReportService {
    asset_service: AssetService,
    renderer: Arc<TimelineRenderer>,
    report_channel_id: Option<ChannelId>,
    tracker: Arc<Mutex<Tracker>>,
}

#[derive(Debug, Clone)]
pub struct RoomDTO {
    pub created_at: Instant,
    pub timestamp: Timestamp,
    pub guild_id: GuildId,
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
            guild_id: room.guild_id(),
            channel_id: room.channel_id(),
            participants,
        }
    }
}

impl ReportService {
    pub fn new(asset_service: AssetService, report_channel_id: Option<ChannelId>) -> Self {
        Self{
            asset_service,
            renderer: Arc::new(TimelineRenderer::new()),
            report_channel_id,
            tracker: Arc::new(Mutex::new(Tracker::new())),
        }
    }

    async fn create_timeline(&self, now: Instant, room: &RoomDTO) -> ReportServiceResult<Timeline> {
        let mut visuals = HashMap::new();

        for participant in &room.participants {
            let visual =
                match self.asset_service.get_members_visual(room.guild_id, participant.user_id(), participant.face()).await {
                    Ok(visual) => visual,
                    Err(err) => return Err(ReportServiceError::AssetError(err)),
                };

            visuals.insert(participant.user_id(), visual);
        }

        Ok(transform(now, room, &visuals))
    }

    pub async fn send_room_report(&self, http: &Http, now: Instant, room: &RoomDTO) -> ReportServiceResult<()> {
        let timeline = self.create_timeline(now, room).await?;

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

        let mut tracker_guard = self.tracker.lock().await;

        let report_channel_id = self.report_channel_id.unwrap_or(room.channel_id.clone());

        match tracker_guard.get_track(&room.channel_id) {
            Some(track) => {
                if track.last_updated_at + Duration::from_secs(20) > now {
                    return Ok(())
                }

                match report_channel_id
                    .edit_message(
                        http,
                        track.message_id,
                        EditMessage::new()
                            .embed(self.renderer.generate_ongoing_embed(
                                now,
                                Timestamp::now(),
                                room,
                            ))
                            .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                            .attachments(EditAttachments::new().add(CreateAttachment::bytes(encoded_image, "thumbnail.png"))),
                    )
                    .await {
                    Ok(_) => {
                        tracker_guard.update_track(room.channel_id);
                        Ok(())
                    },
                    Err(err) => Err(ReportServiceError::GenericError(err.to_string())),
                }
            },
            None => {
                match report_channel_id
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
                    Ok(message) => {
                        tracker_guard.add_track(room.channel_id, message.id);
                        Ok(())
                    },
                    Err(err) => Err(ReportServiceError::GenericError(err.to_string())),
                }
            }
        }

    }
}
