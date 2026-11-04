use crate::graphics::timeline;
use crate::infrastructure::{AssetProvider, MemberVisual};
use crate::reporting::transformer::transform;
use crate::reporting::types::RoomSnapshot;
use crate::reporting::{ParticipantSnapshot, ReportAnchor, transformer};
use crate::room::{Moment, SessionEvent};
use anyhow::Context;
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
use std::io::Cursor;
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
    image_format: image::ImageFormat,
}

impl Reporter {
    /// Parses a config string into an `image::ImageFormat`.
    pub fn parse_image_format(s: &str) -> image::ImageFormat {
        match s.to_lowercase().as_str() {
            "png" => image::ImageFormat::Png,
            "jpeg" | "jpg" => image::ImageFormat::Jpeg,
            "gif" => image::ImageFormat::Gif,
            _ => image::ImageFormat::WebP,
        }
    }
    fn image_file_name(&self) -> &'static str {
        match self.image_format {
            image::ImageFormat::Png => "timeline.png",
            image::ImageFormat::Jpeg => "timeline.jpg",
            image::ImageFormat::Gif => "timeline.gif",
            _ => "timeline.webp",
        }
    }

    fn image_file_url(&self) -> String {
        format!("attachment://{}", self.image_file_name())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        http: Arc<Http>,
        cache: Arc<Cache>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
        timezone: Tz,
        image_format: image::ImageFormat,
    ) -> Self {
        Self {
            http,
            cache,
            asset_provider,
            renderer,
            session_event_rx,
            anchor,
            timezone,
            image_format,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        http: Arc<Http>,
        cache: Arc<Cache>,
        asset_provider: AssetProvider,
        renderer: timeline::Renderer,
        session_event_rx: broadcast::Receiver<SessionEvent>,
        anchor: ReportAnchor,
        timezone: Tz,
        image_format: image::ImageFormat,
    ) {
        let reporter = Self::new(
            http,
            cache,
            asset_provider,
            renderer,
            session_event_rx,
            anchor,
            timezone,
            image_format,
        );
        tokio::spawn(reporter.run());
    }

    fn generate_report<'a>(
        title: &'a str,
        description: &'a str,
        history: &str,
        timestamp: Timestamp,
        elapsed: (Duration, Duration),
        accent_color: Colour,
        image_url: &'a str,
    ) -> Report<'a> {
        Report {
            title: CreateTextDisplay::new(title),
            description: CreateTextDisplay::new(description),
            timeline: CreateMediaGallery::new(vec![CreateMediaGalleryItem::new(
                CreateUnfurledMediaItem::new(image_url),
            )]),
            history: CreateContainer::new(vec![CreateContainerComponent::TextDisplay(
                CreateTextDisplay::new(format!("### 履歴\n{}", history)),
            )])
            .accent_color(accent_color),
            footer: CreateTextDisplay::new(format!(
                "-# ringring-rs v26.4.12 {}\n-# rendering {:.1}ms / encoding {:.1}ms",
                FormattedTimestamp::new(timestamp, Some(FormattedTimestampStyle::RelativeTime)),
                elapsed.0.as_secs_f64() * 1000.0,
                elapsed.1.as_secs_f64() * 1000.0,
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
                    names.get(&p.id).map(String::as_str).unwrap_or("")
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
    ) -> HashMap<UserId, MemberVisual> {
        let mut visuals = HashMap::new();
        for participant in participants {
            match asset_provider.get_members_visual(guild_id, participant.id).await {
                Ok(visual) => {
                    visuals.insert(participant.id, visual);
                }
                Err(error) => {
                    warn!(
                        user_id = %participant.id,
                        error = %error,
                        "Failed to fetch member visual, using fallback"
                    );
                    visuals.insert(participant.id, MemberVisual::fallback(participant.id));
                }
            }
        }
        visuals
    }

    async fn perform_report(
        &mut self,
        snapshot: &RoomSnapshot,
        now: Moment,
        ongoing: bool,
    ) -> anyhow::Result<()> {
        let visuals = Self::fetch_member_visuals(
            &self.asset_provider,
            snapshot.guild_id,
            snapshot.participants.as_ref(),
        )
        .await;

        let elapsed =
            now.mono.checked_duration_since(snapshot.start.mono).and_then(|d| TimeDelta::from_std(d).ok()).unwrap_or(TimeDelta::zero());
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
        let image_format = self.image_format;

        let task: Result<_, anyhow::Error> = tokio::task::spawn_blocking(move || {
            let start = Instant::now();
            let img = renderer.generate_raw_image(timeline);
            let end_rendering = Instant::now();

            let mut bytes: Vec<u8> = Vec::new();
            img.write_to(&mut Cursor::new(&mut bytes), image_format)
                .context("failed to encode image")?;
            let end_encoding = Instant::now();
            Ok((bytes, end_rendering - start, end_encoding - end_rendering))
        })
        .await
        .context("failed to spawn blocking task")?;

        let (image, rendering_elapsed, encoding_elapsed) = task.context("failed to render")?;

        let image_url = self.image_file_url();
        let image_file = self.image_file_name();

        let report = Self::generate_report(
            &title,
            &description,
            &history,
            now.wall,
            (rendering_elapsed, encoding_elapsed),
            colour,
            &image_url,
        );

        self.anchor
            .sync(
                &self.http,
                report.generate_component(),
                CreateAttachment::bytes(image, image_file),
            )
            .await
            .context("failed to sync anchor")?;

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
                            Ok(SessionEvent::PreUpdate) => {
                                // No-op: the broadcast buffer has already evicted
                                // the previous Updated's Arc reference when PreUpdate
                                // was sent.  The Room task's Arc::make_mut can now
                                // mutate in place.  We do not hold an Arc here
                                // (RoomSnapshot owns its Vecs), so there is
                                // nothing to drop.
                            }
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
                                // since we usually send PreUpdate and Update sequentially,
                                // the actual lagged event will be the half of (skipped - 1).
                                let actual_skipped = (skipped.saturating_sub(1)) / 2;
                                if actual_skipped > 0 {
                                    warn!(
                                        "Reporter lagged behind, skipped {} events. Catching up.",
                                        skipped
                                    );
                                }
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
