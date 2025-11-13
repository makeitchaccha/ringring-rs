mod policy;
mod layout;

use crate::model::Participant;
use crate::service::renderer::timeline::layout::{LayoutConfig, Margin};
use crate::service::renderer::timeline::policy::AspectRatioPolicy;
use crate::service::renderer::view::{FillStyle, Timeline};
use crate::service::report::RoomDTO;
use chrono::TimeDelta;
use serenity::all::{
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, FormattedTimestamp,
    FormattedTimestampStyle, Mentionable, Timestamp,
};
use tiny_skia::{Color, FillRule, FilterQuality, LineCap, Mask, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, Rect, Shader, SpreadMode, Stroke, Transform};
use tokio::time::Instant;

const TIMELINE_BAR_HEIGHT_RATIO: f32 = 4.0 / 7.0;
const TIMELINE_BAR_TOP_RATIO: f32 = 3.0 / 14.0;

const TIMELINE_BAR_BOTTOM_RATIO: f32 = TIMELINE_BAR_TOP_RATIO + TIMELINE_BAR_HEIGHT_RATIO;

const STROKE_WIDTH: f32 = 2.0;
const STREAMING_STROKE_WIDTH: f32 = 5.0;

const HATCH_SIZE: u32 = 10;
const HATCH_LINE_WIDTH: f32 = 3.0;
const MUTED_ALPHA: f32 = 0.8;

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

        // Render fills first.
        for (i, entry) in timeline.entries.iter().enumerate() {
            let headline_bb = layout.headline_bb(i);

            let avatar = entry.avatar.as_ref();

            let center = ((headline_bb.left() + headline_bb.right()) / 2.0, (headline_bb.top() + headline_bb.bottom()) / 2.0);
            let transform = Transform::from_translate(center.0 - layout.avatar_size()/2.0, center.1 - layout.avatar_size()/2.0);

            let avatar_transform = transform.pre_scale(layout.avatar_size()/avatar.width() as f32, layout.avatar_size()/avatar.height() as f32);

            pixmap.draw_pixmap(0, 0, avatar, &paint, avatar_transform, Some(&avatar_mask(transform)));

            let timeline_bb = layout.timeline_bb(i);
            let transformer = Transform::from_bbox(timeline_bb);

            let hatching_pixmap = create_hatching_pattern(entry.fill_color);
            let hatching_shader = Pattern::new(hatching_pixmap.as_ref(), SpreadMode::Repeat, FilterQuality::Bicubic, 1.0, Transform::identity());
            let solid_shader = Shader::SolidColor(entry.fill_color);
            for section in &entry.sections {
                let mut paint = Paint::default();
                paint.anti_alias = true;
                paint.shader = match section.fill_style {
                    FillStyle::Active => solid_shader.clone(),
                    FillStyle::Muted => hatching_shader.clone(),
                    FillStyle::Deafened => { continue }, // skips rendering strokes.
                };

                let path = {
                    let mut path_builder = PathBuilder::new();
                    path_builder.push_rect(Rect::from_ltrb(
                        section.start_ratio,
                        TIMELINE_BAR_TOP_RATIO,
                        section.end_ratio,
                        TIMELINE_BAR_BOTTOM_RATIO,
                    ).unwrap().transform(transformer).unwrap());
                    path_builder.finish().unwrap()
                };

                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            }


            let mut stroke = Stroke::default();
            stroke.line_cap = LineCap::Round;
            stroke.width = STROKE_WIDTH;

            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(entry.fill_color);

            // normal strokes later: they may overlap the previous rendered fills.
            for section in &entry.sections {
                let path = {
                    let mut path_builder = PathBuilder::new();
                    path_builder.push_rect(Rect::from_ltrb(section.start_ratio, TIMELINE_BAR_TOP_RATIO, section.end_ratio, TIMELINE_BAR_BOTTOM_RATIO).unwrap());

                    path_builder.finish().unwrap().transform(transformer).unwrap()
                };

                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }

            let mut stroke = Stroke::default();
            stroke.line_cap = LineCap::Round;
            stroke.width = STREAMING_STROKE_WIDTH;

            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(Color::from_rgba(1.0, 0.4, 0.4, 1.0).unwrap());

            // finally, streaming strokes
            for section in &entry.streaming_sections {
                let path = {
                    let mut path_builder = PathBuilder::new();
                    path_builder.push_rect(Rect::from_ltrb(section.start_ratio, TIMELINE_BAR_TOP_RATIO, section.end_ratio, TIMELINE_BAR_BOTTOM_RATIO).unwrap());

                    path_builder.finish().unwrap().transform(transformer).unwrap()
                };

                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }

        Ok(pixmap)
    }

    pub fn generate_ongoing_embed(
        &self,
        now: Instant,
        timestamp: Timestamp,
        room: &RoomDTO,
    ) -> CreateEmbed {
        let elapsed = TimeDelta::from_std(now - room.created_at).unwrap();

        let builder = CreateEmbed::new()
            .author(CreateEmbedAuthor::new("ringring-rs"))
            .title("On call")
            .description(format!("Room is active on {}", room.channel_id.mention()))
            .field(
                "start",
                format!(
                    "{}",
                    FormattedTimestamp::new(
                        room.timestamp,
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
                Self::format_history(now, &room.participants),
                false,
            )
            .image("attachment://thumbnail.png")
            .timestamp(timestamp)
            .footer(CreateEmbedFooter::new("ringring-rs v25.11.10"));

        builder
    }
}

fn create_hatching_pattern(color: Color) -> Pixmap {
    let size = HATCH_SIZE;
    let mut pixmap = Pixmap::new(size, size).unwrap();
    pixmap.fill(Color::from_rgba8(0, 0, 0, 0)); // 背景は透明

    let mut path_builder = PathBuilder::new();

    const fn over(x: f32) -> f32 {
        x + HATCH_LINE_WIDTH
    }

    const fn under(x: f32) -> f32 {
        x - HATCH_LINE_WIDTH
    }

    // crossline
    path_builder.move_to(under(0.0), over(size as f32));
    path_builder.line_to(over(size as f32), under(0.0));

    // upper
    path_builder.move_to(under(0.0), over(0.0));
    path_builder.line_to(over(0.0), under(0.0));

    // lower
    path_builder.move_to(under(size as f32), over(size as f32));
    path_builder.line_to(over(size as f32), under(size as f32));

    let path = path_builder.finish().unwrap();

    let mut paint = Paint::default();
    paint.anti_alias = true;
    let hatch_color = Color::from_rgba(color.red(), color.green(), color.blue(), MUTED_ALPHA).unwrap();
    paint.set_color(hatch_color);

    let mut stroke = Stroke::default();
    stroke.width = HATCH_LINE_WIDTH;
    stroke.line_cap = LineCap::Butt;

    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

    pixmap
}