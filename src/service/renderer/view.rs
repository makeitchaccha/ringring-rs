use crate::model::VoiceStateFlags;
use crate::service::renderer::view::FillStyle::{Active, Deafened, Muted};
use tiny_skia::{Color, Pixmap};
use tokio::time::Instant;

pub struct Timeline {
    pub start: Instant,
    pub end: Instant,
    pub indicator: Option<Instant>,
    pub entries: Vec<TimelineEntry>,
}

pub struct TimelineEntry {
    pub avatar: Pixmap,
    pub sections: Vec<RenderSection>,
    pub streaming_sections: Vec<StreamingSection>,
    pub active_color: Color,
    pub inactive_color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillStyle {
    Active,
    Muted,
    Deafened,
}

impl FillStyle {
    pub fn from_flags(flags: VoiceStateFlags) -> FillStyle {
        match (flags.is_deafened, flags.is_muted) {
            (true, _) => Deafened,
            (_, true) => Muted,
            (_, _) => Active,
        }
    }
}

pub struct RenderSection {
    pub start_ratio: f32,
    pub end_ratio: f32,
    pub fill_style: FillStyle,
}

pub struct StreamingSection {
    pub start_ratio: f32,
    pub end_ratio: f32,
}
