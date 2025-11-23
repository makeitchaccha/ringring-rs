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
use serenity::prelude::SerenityError;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::JoinError;
use tokio::time::Instant;

#[derive(Debug, Error)]
pub enum ReportServiceError{
    #[error(transparent)]
    Rendering(#[from] TimelineRendererError),

    #[error(transparent)]
    Asset(#[from] Arc<AssetError>),

    #[error("")]
    Join(#[from] JoinError),

    #[error("Serenity error")]
    Serenity(#[from] SerenityError),
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
            tracker: Arc::new(Mutex::new(Tracker::new()))
        }
    }

    async fn create_timeline(&self, now: Instant, room: &RoomDTO, finalized: bool) -> ReportServiceResult<Timeline> {
        let mut visuals = HashMap::new();

        for participant in &room.participants {
            let visual = self.asset_service.get_members_visual(room.guild_id, participant.user_id(), participant.face()).await?;

            visuals.insert(participant.user_id(), visual);
        }

        Ok(transform(now, room, &visuals, finalized))
    }

    pub async fn send_room_report(&self, http: &Http, now: Instant, room: &RoomDTO, ongoing: bool) -> ReportServiceResult<()> {
        let timeline = self.create_timeline(now, room, ongoing).await?;

        let renderer = self.renderer.clone();

        let task = tokio::task::spawn_blocking(move || {
            return renderer.generate_png_image(&timeline);
        });

        let encoded_image = task.await??;


        let mut tracker_guard = self.tracker.lock().await;

        let report_channel_id = self.report_channel_id.unwrap_or(room.channel_id.clone());

        match tracker_guard.get_track(&room.channel_id) {
            Some(track) => {
                if !ongoing && track.last_updated_at + Duration::from_secs(20) > now {
                    return Ok(())
                }

                let report_channel_id = self.report_channel_id.unwrap_or(room.channel_id.clone());

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
                        if ongoing {
                            tracker_guard.update_track(room.channel_id);
                        } else {
                            tracker_guard.remove(room.channel_id);
                        }
                        Ok(())
                    },
                    Err(err) => Err(err.into()),
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
                        if ongoing {
                            tracker_guard.add_track(room.channel_id, message.id);
                        }
                        Ok(())
                    },
                    Err(err) => Err(err.into()),
                }
            }
        }

    }
}
