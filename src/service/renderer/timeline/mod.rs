mod policy;
mod layout;

use crate::model::{Participant, Room};
use crate::service::renderer::timeline::layout::{LayoutConfig, Margin};
use crate::service::renderer::timeline::policy::AspectRatioPolicy;
use crate::service::renderer::view::Timeline;
use chrono::TimeDelta;
use serenity::all::{
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, FormattedTimestamp,
    FormattedTimestampStyle, Mentionable, Timestamp,
};
use tiny_skia::{Color, FillRule, FilterQuality, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};
use tokio::time::Instant;

#[derive(Debug)]
pub enum TimelineRendererError {
    PixelmapCreationError,
}

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
                avatar_size: 64.0,
                aspect_ratio_policy: AspectRatioPolicy::discord_thumbnail_4_3(),
            }
        }
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

    pub fn generate_image(&self, timeline: &Timeline) -> TimelineRendererResult<Pixmap> {
        let n_entries = timeline.entries.len();
        let layout = self.layout_config.calculate(n_entries);


        let path = {
            let mut path_builder = PathBuilder::new();
            path_builder.push_circle(layout.avatar_size()/2.0, layout.avatar_size()/2.0, layout.avatar_size()/2.0);
            path_builder.push_circle(layout.avatar_size()/2.0, layout.avatar_size()/2.0, 0.0);
            path_builder.finish().unwrap()
        };

        let avatar_mask = |transform| {
            let mut avatar_mask = Mask::new(layout.total_width() as u32, layout.total_height() as u32).unwrap();
            avatar_mask.fill_path(&path, FillRule::EvenOdd, true, transform);
            avatar_mask
        };


        let mut pixmap = Pixmap::new(layout.total_width() as u32, layout.total_height() as u32)
            .ok_or(TimelineRendererError::PixelmapCreationError)?;
        pixmap.fill(Color::WHITE);

        let mut paint = PixmapPaint::default();
        paint.quality = FilterQuality::Bicubic;

        for (i, entry) in timeline.entries.iter().enumerate() {
            let headline_bb = layout.headline_bb(i);

            let avatar = entry.avatar.as_ref();

            let center = ((headline_bb.left() + headline_bb.right()) / 2.0, (headline_bb.top() + headline_bb.bottom()) / 2.0);
            let transform = Transform::from_translate(center.0 - layout.avatar_size()/2.0, center.1 - layout.avatar_size()/2.0);

            let avatar_transform = transform.pre_scale(layout.avatar_size()/avatar.width() as f32, layout.avatar_size()/avatar.height() as f32);

            pixmap.draw_pixmap(0, 0, avatar, &paint, avatar_transform, Some(&avatar_mask(transform)));

            let timeline_bb = layout.timeline_bb(i);
            let transformer = Transform::from_bbox(timeline_bb);

            for section in &entry.sections {
                let mut path_builder = PathBuilder::new();

                path_builder.push_rect(Rect::from_ltrb(
                    section.start_ratio,
                    5.0/14.0,
                    section.end_ratio,
                    9.0/14.0,
                ).unwrap());

                let path = path_builder.finish().unwrap();

                let mut paint = Paint::default();
                paint.set_color(entry.color);
                paint.anti_alias = true;

                pixmap.fill_path(&path, &paint, FillRule::Winding, transformer, None);
            }
        }

        Ok(pixmap)
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
            .image("attachment://thumbnail.png")
            .timestamp(timestamp)
            .footer(CreateEmbedFooter::new("ringring-rs v25.11.10"));

        builder
    }
}
