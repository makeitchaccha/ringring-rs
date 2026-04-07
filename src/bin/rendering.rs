use ringring_rs::graphics::timeline::Renderer;
use ringring_rs::graphics::timeline::layout::LayoutConfig;
use ringring_rs::graphics::util::Calligraphy;
use ringring_rs::graphics::{Timeline, TimelineEntry};
use ringring_rs::reporting::transformer;
use ringring_rs::room::Moment;
use std::io::Write;
use std::time::Duration;
use tiny_skia::{Color, Pixmap};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let renderer = Renderer::new(LayoutConfig::default(), Calligraphy::default());

    let created_at = Moment::now();

    let mut duration = Duration::from_secs(10);

    for i in 1..1000 {
        if duration > Duration::from_hours(24 * 30) {
            break;
        }

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

        let bytes = renderer.generate_png_image(timeline)?;
        let file_name = format!("./anim/{}.png", i);
        let mut file = std::fs::File::create(file_name)?;
        file.write_all(bytes.as_slice())?;

        duration = duration.mul_f32(1.05);
    }

    Ok(())
}
