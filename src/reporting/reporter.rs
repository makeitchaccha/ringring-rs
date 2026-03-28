use crate::graphics::{Timeline, timeline};
use crate::infrastructure::{AssetProvider, MemberVisual};
use crate::reporting::transformer::transform;
use crate::reporting::types::RoomSnapshot;
use crate::reporting::{ParticipantSnapshot, ReportAnchor};
use crate::room::SessionEvent;
use chrono::TimeDelta;
use serenity::all::{
    CreateAttachment, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateMessage,
    EditAttachments, EditMessage, FormattedTimestamp, FormattedTimestampStyle, GuildId, Http,
    Mentionable, MessageFlags, Timestamp, UserId,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tracing::{error, info, warn};

pub struct Reporter {
    http: Arc<Http>,
    asset_provider: AssetProvider,
    renderer: timeline::Renderer,
    session_event_rx: broadcast::Receiver<SessionEvent>,
    anchor: ReportAnchor,
}

impl Reporter {
    const TIMELINE_IMAGE_FILE: &'static str = "timeline.png";
    const TIMELINE_IMAGE_URL: &'static str =
        constcat::concat!("attachment://", Reporter::TIMELINE_IMAGE_FILE);

    pub fn new(
        http: Arc<Http>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
    ) -> Self {
        Self {
            http,
            asset_provider,
            renderer,
            session_event_rx,
            anchor,
        }
    }

    pub fn spawn(
        http: Arc<Http>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
    ) {
        let reporter = Self::new(http, asset_provider, renderer, session_event_rx, anchor);
        tokio::spawn(reporter.run());
    }

    fn generate_core_embed(timestamp: Timestamp) -> CreateEmbed {
        CreateEmbed::new()
            .author(CreateEmbedAuthor::new("ringring-rs"))
            .image(Self::TIMELINE_IMAGE_URL)
            .timestamp(timestamp)
            .footer(CreateEmbedFooter::new("ringring-rs v26.03.27"))
    }

    fn format_time_delta(delta: TimeDelta) -> String {
        let total_seconds = delta.num_minutes();
        let hours = total_seconds / 60;
        let minutes = total_seconds % 60;

        format!("{:01}:{:02}", hours, minutes)
    }

    fn format_history(now: Instant, participants: &[ParticipantSnapshot]) -> String {
        participants
            .iter()
            .map(|participant| {
                format!(
                    "{} ({})",
                    participant.identity.name,
                    Self::format_time_delta(
                        TimeDelta::from_std(participant.calculate_duration(now)).unwrap()
                    )
                )
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    async fn fetch_member_visuals(
        asset_provider: &AssetProvider,
        guild_id: GuildId,
        participants: &[ParticipantSnapshot],
    ) -> Result<HashMap<UserId, MemberVisual>, String> {
        let mut visuals = HashMap::new();
        for participant in participants {
            let Ok(visual) = asset_provider
                .get_members_visual(
                    guild_id,
                    participant.identity.user_id,
                    &participant.identity.face,
                )
                .await
            else {
                // TODO: fallback
                return Err(format!(
                    "Cannot fetch member visual: {}",
                    participant.identity.user_id
                ));
            };

            visuals.insert(participant.identity.user_id, visual);
        }
        Ok(visuals)
    }

    pub async fn run(mut self) {
        let mut room_snapshot = None;

        loop {
            match self.session_event_rx.recv().await {
                Ok(SessionEvent::Updated { room }) => {
                    let snapshot = RoomSnapshot::from_lease(room);
                    let now = Instant::now();
                    let elapsed = TimeDelta::from_std(now - snapshot.start.mono).unwrap();
                    let embed = Self::generate_core_embed(snapshot.start.wall)
                        .title("On Call")
                        .description(format!(
                            "Room is active on {}",
                            snapshot.channel_id.mention()
                        ))
                        .field(
                            "start",
                            FormattedTimestamp::new(
                                snapshot.start.wall,
                                Some(FormattedTimestampStyle::ShortTime),
                            )
                            .to_string(),
                            true,
                        )
                        .field("elapse", Self::format_time_delta(elapsed).to_string(), true)
                        .field(
                            "history",
                            Self::format_history(now, snapshot.participants.as_ref()),
                            false,
                        );

                    let Ok(visuals) = Self::fetch_member_visuals(
                        &self.asset_provider,
                        snapshot.guild_id,
                        snapshot.participants.as_ref(),
                    )
                    .await
                    else {
                        warn!("failed to fetch member visuals");
                        continue;
                    };

                    let renderer = self.renderer.clone();
                    let timeline = transform(now, &snapshot, &visuals, true);
                    let Ok(task) =
                        tokio::task::spawn_blocking(move || renderer.generate_png_image(&timeline))
                            .await
                    else {
                        error!("failed to spawn blocking task to generate image");
                        continue;
                    };

                    room_snapshot = Some(snapshot);

                    let Ok(image) = task else {
                        error!("failed to generate image");
                        continue;
                    };

                    let _ = self
                        .anchor
                        .sync(
                            &self.http,
                            embed,
                            CreateAttachment::bytes(image, Self::TIMELINE_IMAGE_URL),
                        )
                        .await;
                }
                Ok(SessionEvent::Shutdown { room, end }) => {
                    let snapshot = RoomSnapshot::from_lease(room);
                    let now = Instant::now();
                    let elapsed = TimeDelta::from_std(now - snapshot.start.mono).unwrap();
                    let embed = Self::generate_core_embed(snapshot.start.wall)
                        .title("On Call")
                        .description(format!(
                            "Room is active on {}",
                            snapshot.channel_id.mention()
                        ))
                        .field(
                            "start",
                            FormattedTimestamp::new(
                                snapshot.start.wall,
                                Some(FormattedTimestampStyle::ShortTime),
                            )
                            .to_string(),
                            true,
                        )
                        .field(
                            "end",
                            FormattedTimestamp::new(
                                end.wall,
                                Some(FormattedTimestampStyle::ShortTime),
                            )
                            .to_string(),
                            true,
                        )
                        .field("elapse", Self::format_time_delta(elapsed).to_string(), true)
                        .field(
                            "history",
                            Self::format_history(now, snapshot.participants.as_ref()),
                            false,
                        );

                    let Ok(visuals) = Self::fetch_member_visuals(
                        &self.asset_provider,
                        snapshot.guild_id,
                        snapshot.participants.as_ref(),
                    )
                    .await
                    else {
                        warn!("failed to fetch member visuals");
                        continue;
                    };

                    let renderer = self.renderer.clone();
                    let timeline = transform(now, &snapshot, &visuals, true);
                    let Ok(task) =
                        tokio::task::spawn_blocking(move || renderer.generate_png_image(&timeline))
                            .await
                    else {
                        error!("failed to spawn blocking task to generate image");
                        continue;
                    };

                    room_snapshot = Some(snapshot);

                    let Ok(image) = task else {
                        error!("failed to generate image");
                        continue;
                    };

                    let _ = self
                        .anchor
                        .sync(
                            &self.http,
                            embed,
                            CreateAttachment::bytes(image, Self::TIMELINE_IMAGE_URL),
                        )
                        .await;
                }
                Err(_) => {
                    info!("Session event rx closed, shutdown reporter");
                    break;
                }
            }
        }
    }
}
