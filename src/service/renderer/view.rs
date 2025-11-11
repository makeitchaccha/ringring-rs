use crate::model::VoiceStateFlags;
use crate::service::renderer::view::FillStyle::{Active, Deafened, Muted};
use crate::service::renderer::view::StrokeStyle::{Default, Streaming};
use tiny_skia::Pixmap;
use tokio::time::Instant;

pub struct Timeline {
    pub start: Instant,
    pub end: Instant,
    pub indicator: Option<Instant>,
    pub entries: Vec<TimelineEntry>,
}

pub struct TimelineEntry {
    pub avatar: Pixmap,
    pub name: String,
    pub sections: Vec<RenderSection>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeStyle {
    Default,
    Streaming,
}

impl StrokeStyle {
    pub fn from_flags(flags: VoiceStateFlags) -> StrokeStyle {
        match flags.is_sharing_screen {
            true => Streaming,
            _ => Default,
        }
    }
}

pub struct RenderSection {
    pub start: Instant,
    pub end: Instant,
    pub fill_style: FillStyle,
    pub stroke_style: StrokeStyle,

    pub stroke_left_end: bool,
    pub stroke_right_end: bool,
}
