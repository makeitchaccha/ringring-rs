use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use tiny_skia::{
    Color, FilterQuality, IntSize, Paint, Pattern, Pixmap, PixmapPaint, PixmapRef, Point, Rect,
    SpreadMode, Transform,
};
use tracing::warn;

#[derive(Clone)]
pub struct Calligraphy {
    pub inner: Arc<Mutex<(FontSystem, SwashCache)>>,
}

impl Default for Calligraphy {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new((FontSystem::new(), SwashCache::new()))),
        }
    }
}

pub struct TextSpec<'a> {
    pub content: Cow<'a, str>,
    pub font_size: f32,
    pub pos: Point,
    pub color: Color,
}

impl Calligraphy {
    pub fn draw_text(&self, pixmap: &mut Pixmap, texts: &[TextSpec]) -> Result<(), &'static str> {
        let mut inner_guard = self.inner.lock().map_err(|_| "Mutex poisoned")?;
        let (ref mut font_system, ref mut swash_cache) = *inner_guard;

        for text in texts {
            let metrics = Metrics::new(text.font_size, text.font_size * 1.2);
            let mut buffer = Buffer::new(font_system, metrics);

            let attrs = Attrs::new();
            buffer.set_text(
                font_system,
                text.content.as_ref(),
                &attrs,
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(font_system, true);

            let color_u8 = text.color.to_color_u8();
            for run in buffer.layout_runs() {
                let height = metrics.font_size;
                let Some(text_area_size) =
                    IntSize::from_wh(run.line_w.ceil() as u32, height.ceil() as u32)
                else {
                    warn!(text=%run.text, "Failed to determine size");
                    continue;
                };
                let mut text_alpha_data =
                    vec![0; text_area_size.width() as usize * text_area_size.height() as usize];

                for glyph in run.glyphs {
                    let physical_glyph = glyph.physical((0.0, 0.0), 1.0);

                    if let Some(image) =
                        swash_cache.get_image(font_system, physical_glyph.cache_key)
                    {
                        let left = image.placement.left + physical_glyph.x;
                        let top = height as i32 - image.placement.top + physical_glyph.y;
                        let width = image.placement.width;
                        let height = image.placement.height;

                        if width == 0 || height == 0 {
                            continue;
                        }

                        match image.content {
                            SwashContent::Mask => {
                                // character
                                for (i, &a) in image.data.iter().enumerate() {
                                    let x = i as i32 % width as i32 + left;
                                    let y = i as i32 / width as i32 + top;
                                    if x < 0 || text_area_size.width() as i32 <= x {
                                        continue;
                                    }
                                    if y < 0 || text_area_size.height() as i32 <= y {
                                        continue;
                                    }
                                    let idx = (x + y * text_area_size.width() as i32) as usize;
                                    text_alpha_data[idx] = a;
                                }
                            }

                            SwashContent::Color => {
                                // emoji
                                if let Some(glyph_pixmap) =
                                    PixmapRef::from_bytes(&image.data, width, height)
                                {
                                    pixmap.draw_pixmap(
                                        left,
                                        top,
                                        glyph_pixmap,
                                        &PixmapPaint::default(),
                                        Transform::identity(),
                                        None,
                                    );
                                }
                            }

                            SwashContent::SubpixelMask => {
                                // skips
                            }
                        }
                    }
                }

                let mut text_pixmap_data = Vec::with_capacity(text_alpha_data.len() * 4);
                for a in text_alpha_data {
                    text_pixmap_data.extend_from_slice(&[
                        color_u8.red(),
                        color_u8.green(),
                        color_u8.blue(),
                        a,
                    ]);
                }

                let Some(text_pixmap) = Pixmap::from_vec(text_pixmap_data, text_area_size) else {
                    warn!("failed to construct pixmap from vec");
                    continue;
                };

                let shader = Pattern::new(
                    text_pixmap.as_ref(),
                    SpreadMode::Pad,
                    FilterQuality::Bicubic,
                    1.0,
                    Transform::identity(),
                );

                pixmap.fill_rect(
                    Rect::from_xywh(0.0, 0.0, run.line_w, height).unwrap(),
                    &Paint {
                        shader,
                        anti_alias: true,
                        ..Paint::default()
                    },
                    Transform::from_translate(
                        text.pos.x - run.line_w / 2.0,
                        text.pos.y - metrics.font_size,
                    ),
                    None,
                )
            }
        }

        Ok(())
    }
}
