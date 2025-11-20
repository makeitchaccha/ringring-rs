mod policy;
mod layout;

use crate::model::Participant;
use crate::service::renderer::timeline::layout::{LayoutConfig, Margin};
use crate::service::renderer::timeline::policy::AspectRatioPolicy;
use crate::service::renderer::view::{FillStyle, Timeline};
use crate::service::report::RoomDTO;
use chrono::{DurationRound, TimeDelta};
use cosmic_text::{Align, Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use serenity::all::{
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, FormattedTimestamp,
    FormattedTimestampStyle, Mentionable, Timestamp,
};
use std::sync::{Arc, Mutex};
use tiny_skia::{Color, FillRule, FilterQuality, IntSize, LineCap, Mask, NonZeroRect, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, PixmapRef, Point, Rect, Shader, SpreadMode, Stroke, Transform};
use tokio::time::Instant;
use tracing::debug;

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
    font_system: Arc<Mutex<FontSystem>>,
    swash_cache: Arc<Mutex<SwashCache>>,
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
                label_area_height: 20.0,
                avatar_column_width: 100.0,
                min_timeline_width: 900.0,
                entry_height: 70.0,
                avatar_size: 64.0,
                aspect_ratio_policy: AspectRatioPolicy::discord_thumbnail_4_3(),
            },
            font_system: Arc::new(Mutex::new(FontSystem::new())),
            swash_cache: Arc::new(Mutex::new(SwashCache::new())),
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

        // Render ticks first.
        {
            let mut font_system = self.font_system.lock().unwrap();
            let mut swash_cache = self.swash_cache.lock().unwrap();
            Self::render_ticks(&mut pixmap, timeline, layout.full_timeline_bb(), &mut font_system, &mut swash_cache);

        }

        let mut paint = PixmapPaint::default();
        paint.quality = FilterQuality::Bicubic;

        // Then, Render fills.
        for (i, entry) in timeline.entries.iter().enumerate() {
            let headline_bb = layout.headline_bb_for_entry(i);

            let avatar = entry.avatar.as_ref();

            let center = ((headline_bb.left() + headline_bb.right()) / 2.0, (headline_bb.top() + headline_bb.bottom()) / 2.0);
            let transform = Transform::from_translate(center.0 - layout.avatar_size()/2.0, center.1 - layout.avatar_size()/2.0);

            let avatar_transform = transform.pre_scale(layout.avatar_size()/avatar.width() as f32, layout.avatar_size()/avatar.height() as f32);

            pixmap.draw_pixmap(0, 0, avatar, &paint, avatar_transform, Some(&avatar_mask(transform)));

            let timeline_bb = layout.timeline_bb_for_entry(i);
            let transformer = Transform::from_bbox(timeline_bb);

            let muted_pixmap = create_hatching_pattern(entry.active_color, entry.inactive_color);
            let muted_shader = Pattern::new(muted_pixmap.as_ref(), SpreadMode::Repeat, FilterQuality::Bicubic, 1.0, Transform::identity());
            let active_shader = Shader::SolidColor(entry.active_color);
            let deafened_shader = Shader::SolidColor(entry.inactive_color);

            for section in &entry.voice_sections {
                let mut paint = Paint::default();
                paint.anti_alias = true;
                paint.shader = match section.fill_style {
                    FillStyle::Active => active_shader.clone(),
                    FillStyle::Muted => muted_shader.clone(),
                    FillStyle::Deafened => deafened_shader.clone(),
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
            paint.set_color(entry.active_color);

            let path_creator = |start_ratio, end_ratio| {
                let mut path_builder = PathBuilder::new();
                path_builder.push_rect(Rect::from_ltrb(start_ratio, TIMELINE_BAR_TOP_RATIO, end_ratio, TIMELINE_BAR_BOTTOM_RATIO).unwrap());

                path_builder.finish().unwrap().transform(transformer).unwrap()
            };

            // normal strokes later: they may overlap the previous rendered fills.
            for section in &entry.voice_sections {
                pixmap.stroke_path(&path_creator(section.start_ratio, section.end_ratio), &paint, &stroke, Transform::identity(), None);
            }

            let mut stroke = Stroke::default();
            stroke.line_cap = LineCap::Round;
            stroke.width = STREAMING_STROKE_WIDTH;

            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(entry.streaming_color);

            // finally, streaming strokes
            for section in &entry.streaming_sections {
                pixmap.stroke_path(&path_creator(section.start_ratio, section.end_ratio), &paint, &stroke, Transform::identity(), None);
            }
        }

        // draw start and end
        let path = {
            let mut path_builder = PathBuilder::new();
            path_builder.move_to(0.0, 0.0);
            path_builder.line_to(0.0, 1.0);
            path_builder.move_to(1.0, 0.0);
            path_builder.line_to(1.0, 1.0);

            path_builder.finish().unwrap().transform(Transform::from_bbox(layout.full_timeline_bb())).unwrap()
        };
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba(0.2, 0.2, 0.2, 1.0).unwrap());

        let mut stroke = Stroke::default();
        stroke.width = STREAMING_STROKE_WIDTH;

        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

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

    fn render_ticks(pixmap: &mut Pixmap, timeline: &Timeline, full_timeline_bb: NonZeroRect, font_system: &mut FontSystem, swash_cache: &mut SwashCache) {
        let interval = TimeDelta::from_std(timeline.tick.interval).unwrap();
        let base_timestamp = timeline.created_timestamp.duration_trunc(interval).unwrap();

        let mut delta = base_timestamp - timeline.created_timestamp;
        if delta < TimeDelta::zero() {
            delta += interval;
        }
        let elapsed = TimeDelta::from_std(timeline.terminated_at - timeline.created_at).unwrap();

        let transform = Transform::from_bbox(full_timeline_bb);

        let path = {
            let mut builder = PathBuilder::new();

            while delta < elapsed {
                let ratio = delta.as_seconds_f32()/elapsed.as_seconds_f32();
                let mut position = (ratio, 0.0f32).into();
                transform.map_point(&mut position);
                draw_text(pixmap, font_system, swash_cache, timeline.tick.format(timeline.created_timestamp + delta).as_str(), 20.0, position.x, position.y, Color::BLACK);
                builder.move_to(ratio, 0.0);
                builder.line_to(ratio, 1.0);
                delta += interval;
            }

            builder.finish().unwrap().transform(transform).unwrap()
        };

        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba(0.4, 0.4, 0.4, 1.0).unwrap());
        let mut stroke = Stroke::default();
        stroke.width = 1.0;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn create_hatching_pattern(active: Color, inactive: Color) -> Pixmap {
    let size = HATCH_SIZE;
    let mut pixmap = Pixmap::new(size, size).unwrap();
    pixmap.fill(inactive);

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
    let hatch_color = Color::from_rgba(active.red(), active.green(), active.blue(), MUTED_ALPHA).unwrap();
    paint.set_color(hatch_color);

    let mut stroke = Stroke::default();
    stroke.width = HATCH_LINE_WIDTH;
    stroke.line_cap = LineCap::Butt;

    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

    pixmap
}

fn draw_text(
    pixmap: &mut Pixmap,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    text: &str,
    font_size: f32,
    x: f32,
    y: f32,
    color: Color,
) {
    let metrics = Metrics::new(font_size, font_size * 1.2);
    let mut buffer = Buffer::new(font_system, metrics);

    let attrs = Attrs::new();
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, true);

    let size = IntSize::from_wh(pixmap.width(), pixmap.height()).unwrap();
    let mut text_mask_data = vec![0; size.width() as usize * size.height() as usize];

    for run in buffer.layout_runs() {
        let half_line_width = run.line_w / 2.0;

        for glyph in run.glyphs {
            debug!("now drawing: {:?}", glyph);
            let physical_glyph = glyph.physical((-half_line_width, 0.0), 1.0);

            if let Some(image) = swash_cache.get_image(font_system, physical_glyph.cache_key) {
                debug!("placement: {:?}", image.placement);
                let left = x as i32 + image.placement.left + physical_glyph.x;
                let top = y as i32 - image.placement.top + physical_glyph.y;
                let width = image.placement.width;
                let height = image.placement.height;

                if width == 0 || height == 0 {
                    continue;
                }

                match image.content {
                    SwashContent::Mask => { // character
                        for (i, &a) in image.data.iter().enumerate() {
                            let x = i as i32 % width as i32 + left;
                            let y = i as i32 / width as i32 + top;
                            if x < 0 || size.width() as i32 <= x {
                                continue;
                            }
                            if y < 0 || size.height() as i32 <= y {
                                continue;
                            }
                            let idx = (x + y * size.width() as i32) as usize;
                            text_mask_data[idx] = a;
                        }
                    },

                    SwashContent::Color => { // emoji
                        if let Some(glyph_pixmap) = PixmapRef::from_bytes(&image.data, width, height) {
                            pixmap.draw_pixmap(
                                left,
                                top,
                                glyph_pixmap,
                                &PixmapPaint::default(),
                                Transform::identity(),
                                None,
                            );
                        }
                    },

                    SwashContent::SubpixelMask => {
                        // skips
                    }
                }
            }
        }

    }

    let mut paint = Paint::default();
    paint.set_color(color);

    if let Some(mask) = Mask::from_vec(text_mask_data, size) {
        pixmap.fill_rect(
            Rect::from_xywh(0.0, 0.0, size.width() as f32, size.height() as f32)
                .unwrap_or(Rect::from_xywh(0.0, 0.0, 0.0, 0.0).unwrap()),
            &paint,
            Transform::identity(),
            Some(&mask),
        );
    }
}