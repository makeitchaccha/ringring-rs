mod policy;
mod layout;

use crate::model::{Activity, Participant, Room};
use crate::service::renderer::view::{FillStyle, RenderSection, StrokeStyle, Timeline};
use chrono::TimeDelta;
use serenity::all::{
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, EmbedImage, FormattedTimestamp,
    FormattedTimestampStyle, Mentionable, Timestamp,
};
use tiny_skia::{Pixmap, Transform};
use tokio::time::Instant;
use crate::service::renderer::timeline::layout::{LayoutConfig, Margin};
use crate::service::renderer::timeline::policy::AspectRatioPolicy;

pub enum TimelineRendererError {}

pub type TimelineRendererResult<T> = Result<T, TimelineRendererError>;

pub struct TimelineRenderer{
    layout_config: LayoutConfig,
}

impl TimelineRenderer {
    pub fn new() -> TimelineRenderer {
        TimelineRenderer {
            layout_config: LayoutConfig{
                margin: Margin{
                    left: 10.0,
                    top: 10.0,
                    right: 10.0,
                    bottom: 10.0,
                },
                avatar_column_width: 100.0,
                min_timeline_width: 900.0,
                entry_height: 70.0,
                aspect_ratio_policy: AspectRatioPolicy::discord_thumbnail_4_3(),
            }
        }
    }

    fn convert_to_render_sections(now: Instant, start: Instant, end: Instant, history: &Vec<Activity>) -> Vec<RenderSection> {

        let duration_sec = (end - start).as_secs_f32();
        let mut render_sections = Vec::new();

        for i in 0..history.len() {
            let current = &history[i];
            let fill_style = FillStyle::from_flags(current.flags());
            let stroke_style = StrokeStyle::from_flags(current.flags());

            let stroke_left_end = if i == 0 {
                true
            } else {
                let prev = &history[i - 1];
                !current.is_following(prev)
            };

            let stroke_right_end = if i == history.len() - 1 {
                true
            } else {
                let next = &history[i + 1];
                !next.is_following(current)
            };

            let start_ratio = (current.start() - start).as_secs_f32()/duration_sec;
            let end_ratio = (current.end().unwrap_or(now) - start).as_secs_f32()/duration_sec;

            render_sections.push(RenderSection {
                start_ratio,
                end_ratio,
                fill_style,
                stroke_style,
                stroke_left_end,
                stroke_right_end,
            })
        }

        render_sections
    }

    fn format_time_delta(delta: TimeDelta) -> String {
        let total_seconds = delta.num_minutes();
        let hours = total_seconds / 60;
        let minutes = total_seconds % 60;

        format!("{:01}:{:02}", hours, minutes)
    }

    fn format_history(now: Instant, participants: &Vec<Participant>) -> String {
        participants
            .iter()
            .map(|participant| {
                format!(
                    "{} ({})",
                    participant.name(),
                    Self::format_time_delta(
                        TimeDelta::from_std(participant.calculate_duration(now)).unwrap()
                    )
                )
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub fn generate_image(&self, timeline: &Timeline) -> TimelineRendererResult<()> {
        let n_entries = timeline.entries.len();
        let layout = self.layout_config.calculate(n_entries);
        let mut pixmap = Pixmap::new(layout.total_width() as u32, layout.total_height() as u32).unwrap();

        for (i, entry) in timeline.entries.iter().enumerate() {
            let _headline_bb = layout.headline_bb(i);
            let timeline_bb = layout.timeline_bb(i);
            let transformer = Transform::from_bbox(timeline_bb);

            for section in &entry.sections {

            }
        }

        Ok(())
    }

    pub fn generate_ongoing_embed(
        &self,
        now: Instant,
        timestamp: Timestamp,
        room: &Room,
    ) -> CreateEmbed {
        let elapsed = TimeDelta::from_std(now - room.created_at()).unwrap();

        let builder = CreateEmbed::new()
            .author(CreateEmbedAuthor::new("ringring-rs"))
            .title("On call")
            .description(format!("Room is active on {}", room.channel_id().mention()))
            .field(
                "start",
                format!(
                    "{}",
                    FormattedTimestamp::new(
                        room.timestamp(),
                        Some(FormattedTimestampStyle::ShortTime)
                    )
                ),
                true,
            )
            .field(
                "elapsed",
                format!("{}", Self::format_time_delta(elapsed)),
                true,
            )
            .field(
                "history",
                Self::format_history(now, room.participants()),
                false,
            )
            .timestamp(timestamp)
            .footer(CreateEmbedFooter::new("ringring-rs v25.11.10"));

        builder
    }
}
