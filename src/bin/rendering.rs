use image::{ImageFormat, RgbaImage};
use ringring_rs::graphics::timeline::Renderer;
use ringring_rs::graphics::timeline::layout::LayoutConfig;
use ringring_rs::graphics::util::Calligraphy;
use ringring_rs::graphics::{Timeline, TimelineEntry};
use ringring_rs::reporting::transformer;
use ringring_rs::room::Moment;
use std::time::Duration;
use tiny_skia::{Color, Pixmap};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let renderer = Renderer::new(LayoutConfig::default(), Calligraphy::default());

    let created_at = Moment::now();

    let duration = Duration::from_hours(11);

    let terminated_at = created_at.mono + duration;

    let timeline = Timeline {
        created_at: created_at.mono,
        created_timestamp: created_at.wall.naive_local(),
        terminated_at,
        axis: transformer::choose_suitable_axis(duration),
        entries: vec![TimelineEntry {
            avatar: Pixmap::new(1, 1).unwrap(),
            voice_sections: vec![],
            streaming_sections: vec![],
            active_color: Color::BLACK,
            inactive_color: Color::BLACK,
            streaming_color: Color::BLACK,
        }],
    };

    let raw_image = renderer.generate_raw_image(timeline);
    let file_name = "./timeline.webp".to_string();
    let img = RgbaImage::from_raw(raw_image.width, raw_image.height, raw_image.data)
        .expect("malformed image");

    img.save_with_format(file_name, ImageFormat::WebP)?;

    Ok(())
}
