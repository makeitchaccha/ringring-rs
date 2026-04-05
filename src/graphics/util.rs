use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use tiny_skia::{
    Color, IntSize, Mask, Paint, Pixmap, PixmapPaint, PixmapRef, Point, Rect, Transform,
};

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

            let size = IntSize::from_wh(pixmap.width(), pixmap.height()).unwrap();
            let mut text_mask_data = vec![0; size.width() as usize * size.height() as usize];

            let x = text.pos.x as i32;
            let y = text.pos.y as i32;

            for run in buffer.layout_runs() {
                let half_line_width = run.line_w / 2.0;

                for glyph in run.glyphs {
                    let physical_glyph = glyph.physical((-half_line_width, 0.0), 1.0);

                    if let Some(image) =
                        swash_cache.get_image(font_system, physical_glyph.cache_key)
                    {
                        let left = x + image.placement.left + physical_glyph.x;
                        let top = y - image.placement.top + physical_glyph.y;
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
                                    if x < 0 || size.width() as i32 <= x {
                                        continue;
                                    }
                                    if y < 0 || size.height() as i32 <= y {
                                        continue;
                                    }
                                    let idx = (x + y * size.width() as i32) as usize;
                                    text_mask_data[idx] = a;
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
            }

            let mut paint = Paint::default();
            paint.set_color(text.color);

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

        Ok(())
    }
}
