use crate::graphics::timeline;
use crate::infrastructure::{AssetProvider, MemberVisual};
use crate::reporting::transformer::transform;
use crate::reporting::types::RoomSnapshot;
use crate::reporting::{ParticipantSnapshot, ReportAnchor};
use crate::room::{Moment, SessionEvent};
use chrono::TimeDelta;
use serenity::all::{
    CreateAttachment, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, FormattedTimestamp,
    FormattedTimestampStyle, GuildId, Http, Mentionable, Timestamp, UserId,
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

    async fn perform_report(
        &mut self,
        snapshot: &RoomSnapshot,
        now: Moment,
        ongoing: bool,
    ) -> Result<(), String> {
        let elapsed = TimeDelta::from_std(now.mono - snapshot.start.mono).unwrap();
        let mut embed = Self::generate_core_embed(snapshot.start.wall)
            .title(if ongoing { "On Call" } else { "Call Ended" })
            .description(format!(
                "Room is {} on {}",
                if ongoing { "active" } else { "closed" },
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
            );

        if !ongoing {
            embed = embed.field(
                "end",
                FormattedTimestamp::new(now.wall, Some(FormattedTimestampStyle::ShortTime))
                    .to_string(),
                true,
            );
        }

        embed = embed
            .field("elapse", Self::format_time_delta(elapsed).to_string(), true)
            .field(
                "history",
                Self::format_history(now.mono, snapshot.participants.as_ref()),
                false,
            );

        let visuals = Self::fetch_member_visuals(
            &self.asset_provider,
            snapshot.guild_id,
            snapshot.participants.as_ref(),
        )
        .await?;

        let renderer = self.renderer.clone();
        let timeline = transform(now.mono, snapshot, &visuals, ongoing);
        let task = tokio::task::spawn_blocking(move || renderer.generate_png_image(timeline))
            .await
            .map_err(|e| format!("failed to spawn blocking task: {}", e))?;

        let image = task.map_err(|e| format!("failed to generate image: {}", e))?;

        self.anchor
            .sync(
                &self.http,
                embed,
                CreateAttachment::bytes(image, Self::TIMELINE_IMAGE_FILE),
            )
            .await
            .map_err(|e| format!("failed to sync anchor: {}", e))?;

        Ok(())
    }

    pub async fn run(mut self) {
        let mut scheduler = UpdateScheduler::new(
            Duration::from_secs(5),
            Duration::from_secs(20),
            Duration::from_secs(60),
        );
        let mut last_snapshot: Option<RoomSnapshot> = None;

        loop {
            tokio::select! {
                biased;

                res = self.session_event_rx.recv() => {
                    match res {
                        Ok(SessionEvent::Updated { room }) => {
                            last_snapshot = Some(RoomSnapshot::from_lease(room));
                            scheduler.register_event();
                        }
                        Ok(SessionEvent::Shutdown { room, end }) => {
                            let snapshot = RoomSnapshot::from_lease(room);
                            if let Err(e) = self.perform_report(&snapshot, end, false).await {
                                error!("Failed to perform final report: {}", e);
                            }
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!("Reporter lagged behind, skipped {} events. Catching up.", skipped);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Session event rx closed, shutdown reporter");
                            break;
                        }
                    }
                }

                _ = tokio::time::sleep_until(scheduler.next_deadline()) => {
                    if let Some(snapshot) = &last_snapshot {
                        let now = Moment::now();
                        if let Err(e) = self.perform_report(snapshot, now, true).await {
                            error!("Failed to perform regular report: {}", e);
                        }
                    }
                    scheduler.complete();
                }
            }
        }
    }
}

pub struct UpdateScheduler {
    soft_limit: Duration,
    hard_limit: Duration,
    heartbeat: Duration,

    first_dirty_at: Option<Instant>,
    last_event_at: Option<Instant>,
    last_execution_at: Instant,
}

impl UpdateScheduler {
    pub fn new(soft: Duration, hard: Duration, heartbeat: Duration) -> Self {
        Self {
            soft_limit: soft,
            hard_limit: hard,
            heartbeat,
            first_dirty_at: None,
            last_event_at: None,
            last_execution_at: Instant::now(),
        }
    }

    pub fn register_event(&mut self) {
        let now = Instant::now();
        if self.first_dirty_at.is_none() {
            self.first_dirty_at = Some(now);
        }
        self.last_event_at = Some(now);
    }

    pub fn next_deadline(&self) -> Instant {
        match (self.first_dirty_at, self.last_event_at) {
            (Some(first), Some(last)) => {
                let soft = last + self.soft_limit;
                let hard = first + self.hard_limit;
                soft.min(hard)
            }
            _ => self.last_execution_at + self.heartbeat,
        }
    }

    pub fn complete(&mut self) {
        self.last_execution_at = Instant::now();
        self.first_dirty_at = None;
        self.last_event_at = None;
    }
}
