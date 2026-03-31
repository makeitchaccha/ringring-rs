use crate::graphics::timeline::layout::{Layout, LayoutConfig};
use crate::graphics::util::Calligraphy;
use crate::graphics::{FillStyle, Timeline, TimelineEntry};
use chrono::{DurationRound, TimeDelta};
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, NonZeroRect, Paint, PathBuilder, Pattern, Pixmap,
    Rect, Shader, SpreadMode, Stroke, Transform,
};
use tracing::warn;

const TIMELINE_BAR_HEIGHT_RATIO: f32 = 4.0 / 7.0;
const TIMELINE_BAR_TOP_RATIO: f32 = 3.0 / 14.0;

const TIMELINE_BAR_BOTTOM_RATIO: f32 = TIMELINE_BAR_TOP_RATIO + TIMELINE_BAR_HEIGHT_RATIO;

const STROKE_WIDTH: f32 = 2.0;
const STREAMING_STROKE_WIDTH: f32 = 5.0;

const HATCH_SIZE: u32 = 10;
const HATCH_LINE_WIDTH: f32 = 3.0;
const MUTED_ALPHA: f32 = 0.8;

#[derive(Clone)]
pub struct Renderer {
    layout_config: LayoutConfig,
    calligraphy: Calligraphy,
}

impl Renderer {
    pub fn new(layout_config: LayoutConfig, calligraphy: Calligraphy) -> Renderer {
        Renderer {
            layout_config,
            calligraphy,
        }
    }

    pub fn generate_png_image(&self, timeline: Timeline) -> Result<Vec<u8>, &'static str> {
        let n_entries = timeline.entries.len();
        let layout = self.layout_config.calculate(n_entries);

        let mut pixmap = Pixmap::new(layout.total_width() as u32, layout.total_height() as u32)
            .expect("invalid pixmap size");
        pixmap.fill(Color::WHITE);

        // Render ticks first.
        Self::draw_ticks(
            &mut pixmap,
            &timeline,
            layout.full_timeline_bb(),
            &self.calligraphy,
        );

        // Then, Render fills.
        for (i, entry) in timeline.entries.iter().enumerate() {
            Self::draw_avatar(&mut pixmap, entry, &layout, i);

            let timeline_bb = layout.timeline_bb_for_entry(i);
            let transformer = Transform::from_bbox(timeline_bb);

            let muted_pixmap = create_hatching_pattern(entry.active_color, entry.inactive_color);
            let muted_shader = Pattern::new(
                muted_pixmap.as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Bicubic,
                1.0,
                Transform::identity(),
            );
            let active_shader = Shader::SolidColor(entry.active_color);
            let deafened_shader = Shader::SolidColor(entry.inactive_color);

            for section in &entry.voice_sections {
                let paint = Paint {
                    shader: match section.fill_style {
                        FillStyle::Active => active_shader.clone(),
                        FillStyle::Muted => muted_shader.clone(),
                        FillStyle::Deafened => deafened_shader.clone(),
                    },
                    anti_alias: true,
                    ..Default::default()
                };

                let path = {
                    let mut path_builder = PathBuilder::new();
                    path_builder.push_rect(
                        Rect::from_ltrb(
                            section.start_ratio,
                            TIMELINE_BAR_TOP_RATIO,
                            section.end_ratio,
                            TIMELINE_BAR_BOTTOM_RATIO,
                        )
                        .unwrap()
                        .transform(transformer)
                        .unwrap(),
                    );
                    path_builder.finish().unwrap()
                };

                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }

            let stroke = Stroke {
                line_cap: LineCap::Round,
                width: STROKE_WIDTH,
                ..Default::default()
            };

            let paint = Paint {
                shader: Shader::SolidColor(entry.active_color),
                anti_alias: true,
                ..Default::default()
            };

            let path_creator = |start_ratio, end_ratio| {
                let mut path_builder = PathBuilder::new();
                path_builder.push_rect(
                    Rect::from_ltrb(
                        start_ratio,
                        TIMELINE_BAR_TOP_RATIO,
                        end_ratio,
                        TIMELINE_BAR_BOTTOM_RATIO,
                    )
                    .unwrap(),
                );

                path_builder
                    .finish()
                    .unwrap()
                    .transform(transformer)
                    .unwrap()
            };

            // normal strokes later: they may overlap the previous rendered fills.
            for section in &entry.voice_sections {
                pixmap.stroke_path(
                    &path_creator(section.start_ratio, section.end_ratio),
                    &paint,
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }

            let stroke = Stroke {
                line_cap: LineCap::Round,
                width: STREAMING_STROKE_WIDTH,
                ..Default::default()
            };

            let paint = Paint {
                shader: Shader::SolidColor(entry.streaming_color),
                anti_alias: true,
                ..Default::default()
            };

            // finally, streaming strokes
            for section in &entry.streaming_sections {
                pixmap.stroke_path(
                    &path_creator(section.start_ratio, section.end_ratio),
                    &paint,
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }

        // draw start and end
        let path = {
            let mut path_builder = PathBuilder::new();
            path_builder.move_to(0.0, 0.0);
            path_builder.line_to(0.0, 1.0);
            path_builder.move_to(1.0, 0.0);
            path_builder.line_to(1.0, 1.0);

            path_builder
                .finish()
                .unwrap()
                .transform(Transform::from_bbox(layout.full_timeline_bb()))
                .unwrap()
        };

        let paint = Paint {
            shader: Shader::SolidColor(Color::from_rgba(0.2, 0.2, 0.2, 1.0).unwrap()),
            ..Default::default()
        };

        let stroke = Stroke {
            width: STREAMING_STROKE_WIDTH,
            ..Default::default()
        };

        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

        let image = pixmap.encode_png().map_err(|_e| "failed to encode image")?;

        Ok(image)
    }

    fn draw_avatar(pixmap: &mut Pixmap, entry: &TimelineEntry, layout: &Layout, i: usize) {
        let headline_bb = layout.headline_bb_for_entry(i);
        let avatar = entry.avatar.as_ref();

        let scale = layout.avatar_size() / avatar.width() as f32;
        let shader = Pattern::new(
            avatar,
            SpreadMode::Pad,
            FilterQuality::Bicubic,
            1.0,
            Transform::from_scale(scale, scale),
        );

        let radius = layout.avatar_size() / 2.0;

        let center = (
            (headline_bb.left() + headline_bb.right()) / 2.0,
            (headline_bb.top() + headline_bb.bottom()) / 2.0,
        );
        let transform = Transform::from_translate(center.0 - radius, center.1 - radius);

        let circle = PathBuilder::from_circle(radius, radius, radius).unwrap();
        pixmap.fill_path(
            &circle,
            &Paint {
                shader,
                ..Default::default()
            },
            FillRule::Winding,
            transform,
            None,
        );
    }

    fn draw_ticks(
        pixmap: &mut Pixmap,
        timeline: &Timeline,
        full_timeline_bb: NonZeroRect,
        calligraphy: &Calligraphy,
    ) {
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
                let ratio = delta.as_seconds_f32() / elapsed.as_seconds_f32();
                let mut position = (ratio, 0.0f32).into();
                transform.map_point(&mut position);
                if let Err(err) = calligraphy.draw_text(
                    pixmap,
                    timeline
                        .tick
                        .format(timeline.created_timestamp + delta)
                        .as_str(),
                    20.0,
                    position.x,
                    position.y - 5.0,
                    Color::BLACK,
                ) {
                    warn!("Failed to draw tick, skipping: {:?}", err);
                    continue;
                }
                builder.move_to(ratio, 0.0);
                builder.line_to(ratio, 1.0);
                delta += interval;
            }

            builder.finish().unwrap().transform(transform).unwrap()
        };

        let paint = Paint {
            shader: Shader::SolidColor(Color::from_rgba(0.4, 0.4, 0.4, 1.0).unwrap()),
            ..Default::default()
        };
        let stroke = Stroke {
            width: 1.0,
            ..Default::default()
        };
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

    let paint = Paint {
        anti_alias: true,
        shader: Shader::SolidColor(
            Color::from_rgba(active.red(), active.green(), active.blue(), MUTED_ALPHA).unwrap(),
        ),
        ..Default::default()
    };

    let stroke = Stroke {
        width: HATCH_LINE_WIDTH,
        line_cap: LineCap::Butt,
        ..Default::default()
    };

    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

    pixmap
}
