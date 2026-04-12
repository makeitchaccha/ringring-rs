use crate::graphics::timeline::layout::LayoutConfig;
use crate::graphics::timeline::view::TickKind;
use crate::graphics::util::{Calligraphy, TextSpec};
use crate::graphics::{FillStyle, Timeline, TimelineEntry};
use image::RgbaImage;
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, NonZeroRect, Paint, PathBuilder, Pattern, Pixmap,
    PixmapRef, Point, Rect, Shader, SpreadMode, Stroke, Transform,
};
use tracing::warn;

const TIMELINE_BAR_HEIGHT_RATIO: f32 = 4.0 / 7.0;
const TIMELINE_BAR_TOP_RATIO: f32 = 3.0 / 14.0;

const TIMELINE_BAR_BOTTOM_RATIO: f32 = TIMELINE_BAR_TOP_RATIO + TIMELINE_BAR_HEIGHT_RATIO;

const MAJOR_TICK_COLOR: u8 = 0x1a;
const MINOR_TICK_COLOR: u8 = 0x4d;

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

    pub fn generate_raw_image(&self, timeline: Timeline) -> RgbaImage {
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
            let headline_bb = layout.headline_bb_for_entry(i);
            let avatar_center = Point {
                x: (headline_bb.left() + headline_bb.right()) / 2.0,
                y: (headline_bb.top() + (headline_bb.bottom())) / 2.0,
            };
            Self::draw_avatar(
                &mut pixmap,
                entry.avatar.as_ref(),
                avatar_center,
                layout.avatar_size(),
            );

            let timeline_bb = layout.timeline_bb_for_entry(i);
            Self::draw_timeline_row(&mut pixmap, entry, timeline_bb);
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

        RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec())
            .expect("invalid size")
    }

    fn draw_avatar(pixmap: &mut Pixmap, avatar: PixmapRef, center: Point, avatar_size: f32) {
        let scale = avatar_size / avatar.width() as f32;
        let shader = Pattern::new(
            avatar,
            SpreadMode::Pad,
            FilterQuality::Bicubic,
            1.0,
            Transform::from_scale(scale, scale),
        );

        let radius = avatar_size / 2.0;

        let transform = Transform::from_translate(center.x - radius, center.y - radius);

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

    fn draw_timeline_row(pixmap: &mut Pixmap, entry: &TimelineEntry, row_bb: NonZeroRect) {
        let row_transform = Transform::from_bbox(row_bb);
        let active_paint = Paint {
            shader: Shader::SolidColor(entry.active_color),
            ..Default::default()
        };
        let hatching = create_hatching_pattern(entry.active_color, entry.inactive_color);
        let muted_paint = Paint {
            shader: Pattern::new(
                hatching.as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Bicubic,
                1.0,
                Transform::identity(),
            ),
            ..Default::default()
        };
        let deafened_paint = Paint {
            shader: Shader::SolidColor(entry.inactive_color),
            ..Default::default()
        };

        for style in [FillStyle::Active, FillStyle::Muted, FillStyle::Deafened] {
            let mut pb = PathBuilder::new();
            for section in entry
                .voice_sections
                .iter()
                .filter(|s| s.fill_style == style)
            {
                pb.push_rect(
                    Rect::from_ltrb(
                        section.span.start(),
                        TIMELINE_BAR_TOP_RATIO,
                        section.span.end(),
                        TIMELINE_BAR_BOTTOM_RATIO,
                    )
                    .unwrap(),
                );
            }
            if let Some(path) = pb.finish().and_then(|path| path.transform(row_transform)) {
                let paint = match style {
                    FillStyle::Active => &active_paint,
                    FillStyle::Muted => &muted_paint,
                    FillStyle::Deafened => &deafened_paint,
                };
                pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
            }
        }

        let mut pb = PathBuilder::new();
        for section in entry.voice_sections.iter() {
            pb.push_rect(
                Rect::from_ltrb(
                    section.span.start(),
                    TIMELINE_BAR_TOP_RATIO,
                    section.span.end(),
                    TIMELINE_BAR_BOTTOM_RATIO,
                )
                .unwrap(),
            );
        }
        if let Some(path) = pb.finish().and_then(|path| path.transform(row_transform)) {
            pixmap.stroke_path(
                &path,
                &active_paint,
                &Stroke {
                    line_cap: LineCap::Round,
                    width: STROKE_WIDTH,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }

        let mut pb = PathBuilder::new();
        for section in entry.streaming_sections.iter() {
            pb.push_rect(
                Rect::from_ltrb(
                    section.span.start(),
                    TIMELINE_BAR_TOP_RATIO,
                    section.span.end(),
                    TIMELINE_BAR_BOTTOM_RATIO,
                )
                .unwrap(),
            );
        }
        if let Some(path) = pb.finish().and_then(|path| path.transform(row_transform)) {
            pixmap.stroke_path(
                &path,
                &Paint {
                    shader: Shader::SolidColor(entry.streaming_color),
                    anti_alias: true,
                    ..Default::default()
                },
                &Stroke {
                    line_cap: LineCap::Round,
                    width: STREAMING_STROKE_WIDTH,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    fn draw_ticks(
        pixmap: &mut Pixmap,
        timeline: &Timeline,
        full_timeline_bb: NonZeroRect,
        calligraphy: &Calligraphy,
    ) {
        let ticks = timeline.axis.generate_tick(
            timeline.created_timestamp,
            timeline.created_timestamp + (timeline.terminated_at - timeline.created_at),
        );

        let mut major_pb = PathBuilder::new();
        let mut minor_pb = PathBuilder::new();
        let mut labels = Vec::new();

        let transform = Transform::from_bbox(full_timeline_bb);

        for tick in ticks {
            match tick.kind {
                TickKind::Major { format, timestamp } => {
                    major_pb.move_to(tick.ratio, 0.0);
                    major_pb.line_to(tick.ratio, 1.0);

                    let mut point = Point {
                        x: tick.ratio,
                        y: 0.0,
                    };
                    transform.map_point(&mut point);

                    for (i, line) in format.iter().enumerate() {
                        labels.push(TextSpec {
                            content: timestamp.format(line).to_string().into(),
                            font_size: 20.0,
                            pos: point - (0.0, 5.0 + i as f32 * 25.0).into(),
                            color: Color::BLACK,
                        });
                    }
                }
                TickKind::Minor => {
                    minor_pb.move_to(tick.ratio, 0.0);
                    minor_pb.line_to(tick.ratio, 1.0);
                }
            }
        }

        if let Some(path) = major_pb.finish().and_then(|path| path.transform(transform)) {
            pixmap.stroke_path(
                &path,
                &Paint {
                    shader: Shader::SolidColor(Color::from_rgba8(
                        MAJOR_TICK_COLOR,
                        MAJOR_TICK_COLOR,
                        MAJOR_TICK_COLOR,
                        255,
                    )),
                    ..Default::default()
                },
                &Stroke {
                    width: 1.0,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }

        if let Some(path) = minor_pb.finish().and_then(|path| path.transform(transform)) {
            pixmap.stroke_path(
                &path,
                &Paint {
                    shader: Shader::SolidColor(Color::from_rgba8(
                        MINOR_TICK_COLOR,
                        MINOR_TICK_COLOR,
                        MINOR_TICK_COLOR,
                        255,
                    )),
                    ..Default::default()
                },
                &Stroke {
                    width: 0.5,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }

        if !labels.is_empty()
            && let Err(err) = calligraphy.draw_text(pixmap, &labels)
        {
            warn!(error = %err, "failed to draw text");
        }
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
