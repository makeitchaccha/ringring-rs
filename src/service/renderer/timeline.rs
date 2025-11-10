use crate::model::{Activity, Participant, Room};
use crate::service::renderer::view::{FillStyle, RenderSection, StrokeStyle};
use chrono::TimeDelta;
use serenity::all::{
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, FormattedTimestamp, FormattedTimestampStyle,
    Mentionable, Timestamp,
};
use tokio::time::Instant;

pub struct TimelineRenderer;

impl TimelineRenderer {
    pub fn new() -> TimelineRenderer {
        TimelineRenderer {}
    }
    fn convert_to_render_sections(now: Instant, history: &Vec<Activity>) -> Vec<RenderSection> {
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

            render_sections.push(RenderSection {
                start: current.start(),
                end: current.end().unwrap_or(now),
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
