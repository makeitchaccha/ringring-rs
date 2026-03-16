use crate::graphics::{Timeline, TimelineRenderer, TimelineRendererError, transform};
use crate::infrastructure::{AssetError, AssetProvider};
use crate::reporting::ReportStateStore;
use crate::room::{Participant, Room};
use serenity::all::{
    ChannelId, CreateAttachment, CreateMessage, EditAttachments, EditMessage, GuildId, Http,
    MessageFlags, Timestamp,
};
use serenity::prelude::SerenityError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::JoinError;
use tokio::time::Instant;

#[derive(Debug, Error)]
pub enum ReportServiceError {
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

pub struct Reporter {
    asset_provider: AssetProvider,
    renderer: Arc<TimelineRenderer>,
    report_channels: HashMap<ChannelId, ChannelId>,
    states: Arc<Mutex<ReportStateStore>>,
}

#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub created_at: Instant,
    pub timestamp: Timestamp,
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub participants: Vec<Participant>,
}

impl RoomSnapshot {
    pub fn from_room(room: &Room) -> Self {
        let participants = room.participants().to_vec();

        RoomSnapshot {
            created_at: room.created_at(),
            timestamp: room.timestamp(),
            guild_id: room.guild_id(),
            channel_id: room.channel_id(),
            participants,
        }
    }
}

impl Reporter {
    pub fn new(
        asset_service: AssetProvider,
        report_channels: HashMap<ChannelId, ChannelId>,
    ) -> Self {
        Self {
            asset_provider: asset_service,
            renderer: Arc::new(TimelineRenderer::new()),
            report_channels,
            states: Arc::new(Mutex::new(ReportStateStore::new())),
        }
    }

    async fn create_timeline(
        &self,
        now: Instant,
        snapshot: &RoomSnapshot,
        finalized: bool,
    ) -> ReportServiceResult<Timeline> {
        let mut visuals = HashMap::new();

        for participant in &snapshot.participants {
            let visual = self
                .asset_provider
                .get_members_visual(
                    snapshot.guild_id,
                    participant.identification.user_id,
                    &participant.identification.face,
                )
                .await?;

            visuals.insert(participant.identification.user_id, visual);
        }

        Ok(transform(now, snapshot, &visuals, finalized))
    }

    pub async fn send_room_report(
        &self,
        http: &Http,
        now: Instant,
        snapshot: &RoomSnapshot,
        ongoing: bool,
    ) -> ReportServiceResult<()> {
        let timeline = self.create_timeline(now, snapshot, ongoing).await?;

        let renderer = self.renderer.clone();

        let task = tokio::task::spawn_blocking(move || renderer.generate_png_image(&timeline));

        let encoded_image = task.await??;

        let mut states_guard = self.states.lock().await;

        let report_channel_id = self
            .report_channels
            .get(&snapshot.channel_id)
            .unwrap_or(&snapshot.channel_id);

        match states_guard.get(&snapshot.channel_id) {
            Some(state) => {
                if !ongoing && state.last_updated_at + Duration::from_secs(20) > now {
                    return Ok(());
                }

                match report_channel_id
                    .edit_message(
                        http,
                        state.message_id,
                        EditMessage::new()
                            .embed(self.renderer.generate_ongoing_embed(
                                now,
                                Timestamp::now(),
                                snapshot,
                            ))
                            .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                            .attachments(
                                EditAttachments::new()
                                    .add(CreateAttachment::bytes(encoded_image, "thumbnail.png")),
                            ),
                    )
                    .await
                {
                    Ok(_) => {
                        if ongoing {
                            states_guard.touch(snapshot.channel_id);
                        } else {
                            states_guard.remove(snapshot.channel_id);
                        }
                        Ok(())
                    }
                    Err(err) => Err(err.into()),
                }
            }
            None => {
                match report_channel_id
                    .send_message(
                        http,
                        CreateMessage::new()
                            .embed(self.renderer.generate_ongoing_embed(
                                now,
                                Timestamp::now(),
                                snapshot,
                            ))
                            .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                            .add_file(CreateAttachment::bytes(encoded_image, "thumbnail.png")),
                    )
                    .await
                {
                    Ok(message) => {
                        if ongoing {
                            states_guard.insert(snapshot.channel_id, message.id);
                        }
                        Ok(())
                    }
                    Err(err) => Err(err.into()),
                }
            }
        }
    }
}
