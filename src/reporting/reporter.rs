use crate::graphics::timeline;
use crate::infrastructure::{AssetProvider, MemberVisual};
use crate::reporting::transformer::transform;
use crate::reporting::types::RoomSnapshot;
use crate::reporting::{ParticipantSnapshot, ReportAnchor, transformer};
use crate::room::{Moment, SessionEvent};
use better_tokio_select::tokio_select;
use chrono::TimeDelta;
use chrono_tz::Tz;
use serenity::all::{
    Cache, CacheHttp, Colour, CreateAttachment, CreateComponent, CreateContainer,
    CreateContainerComponent, CreateMediaGalleryItem, CreateSeparator, CreateTextDisplay,
    FormattedTimestamp, FormattedTimestampStyle, GuildId, Http, Mentionable, SeparatorSpacingSize,
    Timestamp, UserId, colours,
};
use serenity::builder::{CreateMediaGallery, CreateUnfurledMediaItem};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tracing::{error, info, warn};

struct Report<'a> {
    title: CreateTextDisplay<'a>,
    description: CreateTextDisplay<'a>,
    timeline: CreateMediaGallery<'a>,
    history: CreateContainer<'a>,
    footer: CreateTextDisplay<'a>,
}

impl<'a> Report<'a> {
    fn generate_component(self) -> impl Into<Cow<'a, [CreateComponent<'a>]>> {
        vec![
            CreateComponent::TextDisplay(self.title),
            CreateComponent::Separator(
                CreateSeparator::new()
                    .divider(true)
                    .spacing(SeparatorSpacingSize::Large),
            ),
            CreateComponent::TextDisplay(self.description),
            CreateComponent::MediaGallery(self.timeline),
            CreateComponent::Container(self.history),
            CreateComponent::Separator(
                CreateSeparator::new()
                    .divider(true)
                    .spacing(SeparatorSpacingSize::Large),
            ),
            CreateComponent::TextDisplay(self.footer),
        ]
    }
}

pub struct Reporter {
    http: Arc<Http>,
    cache: Arc<Cache>,
    asset_provider: AssetProvider,
    renderer: timeline::Renderer,
    session_event_rx: broadcast::Receiver<SessionEvent>,
    anchor: ReportAnchor,
    timezone: Tz,
}

impl Reporter {
    const TIMELINE_IMAGE_FILE: &'static str = "timeline.png";
    const TIMELINE_IMAGE_URL: &'static str =
        constcat::concat!("attachment://", Reporter::TIMELINE_IMAGE_FILE);

    pub fn new(
        http: Arc<Http>,
        cache: Arc<Cache>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
        timezone: Tz,
    ) -> Self {
        Self {
            http,
            cache,
            asset_provider,
            renderer,
            session_event_rx,
            anchor,
            timezone,
        }
    }

    pub fn spawn(
        http: Arc<Http>,
        cache: Arc<Cache>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
        timezone: Tz,
    ) {
        let reporter = Self::new(
            http,
            cache,
            asset_provider,
            renderer,
            session_event_rx,
            anchor,
            timezone,
        );
        tokio::spawn(reporter.run());
    }

    fn generate_report<'a>(
        title: &'a str,
        description: &'a str,
        history: &str,
        timestamp: Timestamp,
        rendering_elapsed: Duration,
        accent_color: Colour,
    ) -> Report<'a> {
        Report {
            title: CreateTextDisplay::new(title),
            description: CreateTextDisplay::new(description),
            timeline: CreateMediaGallery::new(vec![CreateMediaGalleryItem::new(
                CreateUnfurledMediaItem::new(Self::TIMELINE_IMAGE_URL),
            )]),
            history: CreateContainer::new(vec![CreateContainerComponent::TextDisplay(
                CreateTextDisplay::new(format!("### 履歴\n{}", history)),
            )])
            .accent_color(accent_color),
            footer: CreateTextDisplay::new(format!(
                "-# ringring-rs v26.4.7 {}\n-# rendering {}ms",
                FormattedTimestamp::new(timestamp, Some(FormattedTimestampStyle::RelativeTime)),
                rendering_elapsed.as_millis(),
            )),
        }
    }

    async fn format_history(
        participants: &[ParticipantSnapshot],
        now: Instant,
        guild_id: GuildId,
        http: impl CacheHttp,
    ) -> String {
        let mut names = HashMap::new();

        for part in participants {
            let name = match guild_id
                .member(&http, part.id)
                .await
                .map(|member| member.display_name().to_owned())
            {
                Ok(name) => name,
                Err(_) => part
                    .id
                    .to_user(&http)
                    .await
                    .map(|user| user.display_name().to_owned())
                    .unwrap_or("".to_string()),
            };
            names.insert(part.id, name);
        }

        participants
            .iter()
            .map(|p| {
                let total_minutes = p.calculate_duration(now).as_secs() / 60;
                let hours = total_minutes / 60;
                let minutes = total_minutes % 60;
                format!(
                    "- ⏳ `{:>2}:{:02}` {}",
                    hours,
                    minutes,
                    names.get(&p.id).unwrap()
                )
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn format_time_delta(delta: TimeDelta) -> String {
        let total_minutes = delta.num_minutes();
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;

        format!("{:01}:{:02}", hours, minutes)
    }

    async fn fetch_member_visuals(
        asset_provider: &AssetProvider,
        guild_id: GuildId,
        participants: &[ParticipantSnapshot],
    ) -> Result<HashMap<UserId, MemberVisual>, String> {
        let mut visuals = HashMap::new();
        for participant in participants {
            let Ok(visual) = asset_provider
                .get_members_visual(guild_id, participant.id)
                .await
            else {
                // TODO: fallback
                return Err(format!("Cannot fetch member visual: {}", participant.id));
            };

            visuals.insert(participant.id, visual);
        }
        Ok(visuals)
    }

    async fn perform_report(
        &mut self,
        snapshot: &RoomSnapshot,
        now: Moment,
        ongoing: bool,
    ) -> Result<(), String> {
        let visuals = Self::fetch_member_visuals(
            &self.asset_provider,
            snapshot.guild_id,
            snapshot.participants.as_ref(),
        )
        .await?;

        let elapsed =
            TimeDelta::from_std(now.mono - snapshot.start.mono).unwrap_or(TimeDelta::zero());
        let history = Self::format_history(
            &snapshot.participants,
            now.mono,
            snapshot.guild_id,
            &(Some(&self.cache), self.http.as_ref()),
        )
        .await;
        let (title, description, timeline, colour) = if ongoing {
            (
                format!("## 📢【通話中】{}", snapshot.channel_id.mention()),
                format!(
                    "**{}**開始 (**{}**経過)",
                    FormattedTimestamp::new(
                        snapshot.start.wall,
                        Some(FormattedTimestampStyle::ShortTime)
                    ),
                    Self::format_time_delta(elapsed)
                ),
                transform(
                    snapshot.start.mono,
                    transformer::calculate_auto_scale(snapshot.start.mono, now.mono),
                    now.mono,
                    snapshot,
                    &visuals,
                    self.timezone,
                ),
                colours::branding::GREEN,
            )
        } else {
            (
                format!("## 🗄️【通話終了】{}", snapshot.channel_id.mention()),
                format!(
                    "**{}**開始~**{}**終了 (**{}**)",
                    FormattedTimestamp::new(
                        snapshot.start.wall,
                        Some(FormattedTimestampStyle::ShortTime)
                    ),
                    FormattedTimestamp::new(now.wall, Some(FormattedTimestampStyle::ShortTime)),
                    Self::format_time_delta(elapsed)
                ),
                transform(
                    snapshot.start.mono,
                    now.mono,
                    now.mono,
                    snapshot,
                    &visuals,
                    self.timezone,
                ),
                colours::branding::WHITE,
            )
        };

        let renderer = self.renderer.clone();

        let (task, rendering_elapsed) = tokio::task::spawn_blocking(move || {
            let start = Instant::now();
            let res = renderer.generate_png_image(timeline);
            (res, start.elapsed())
        })
        .await
        .map_err(|e| format!("failed to spawn blocking task: {}", e))?;

        let image = task.map_err(|e| format!("failed to generate image: {}", e))?;

        let report = Self::generate_report(
            &title,
            &description,
            &history,
            now.wall,
            rendering_elapsed,
            colour,
        );

        self.anchor
            .sync(
                &self.http,
                report.generate_component(),
                CreateAttachment::bytes(image, Self::TIMELINE_IMAGE_FILE),
            )
            .await
            .map_err(|e| format!("failed to sync anchor: {}", e))?;

        Ok(())
    }

    pub async fn run(mut self) {
        let mut scheduler = UpdateScheduler::new(
            Duration::from_secs(5),
            Duration::from_secs(15),
            Duration::from_secs(60),
        );
        let mut last_snapshot: Option<RoomSnapshot> = None;

        loop {
            tokio_select!(
                biased,
                match .. {
                    .. if let res = self.session_event_rx.recv() => {
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
                            Ok(SessionEvent::PreUpdate) => {}
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(
                                    "Reporter lagged behind, skipped {} events. Catching up.",
                                    skipped
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Session event rx closed, shutdown reporter");
                                break;
                            }
                        }
                    }

                    .. if let _ = tokio::time::sleep_until(scheduler.next_deadline()) => {
                        if let Some(snapshot) = &last_snapshot {
                            let now = Moment::now();
                            if let Err(e) = self.perform_report(snapshot, now, true).await {
                                error!("Failed to perform regular report: {}", e);
                            }
                        }
                        scheduler.complete();
                    }
                }
            )
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
