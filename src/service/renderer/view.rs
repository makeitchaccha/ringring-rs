use std::time::Duration;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use crate::model::VoiceStateFlags;
use crate::service::renderer::view::FillStyle::{Active, Deafened, Muted};
use tiny_skia::{Color, Pixmap};
use tokio::time::Instant;

#[derive(Debug, Copy, Clone)]
pub struct Tick {
    pub interval: Duration,
    with_sec: bool,
}

impl Tick {
    pub const fn secs_grain(secs: u64) -> Self {
        Self{
            interval: Duration::from_secs(secs),
            with_sec: true,
        }
    }

    pub const fn mins_grain(mins: u64) -> Self {
        Self{
            interval: Duration::from_mins(mins),
            with_sec: false,
        }
    }

    pub const fn hours_grain(hours: u64) -> Self{
        Self{
            interval: Duration::from_hours(hours),
            with_sec: false,
        }
    }

    pub fn format<T: TimeZone>(&self, timestamp: DateTime<T>) -> String {
        let year = timestamp.year();
        let month = timestamp.month();
        let day = timestamp.day();
        let hours = timestamp.hour();
        let minutes = timestamp.minute();

        let start_of_day = hours == 0 && minutes == 0;
        let start_of_year = year == 0 && start_of_day;

        if self.with_sec {
            let seconds = timestamp.second();

            match (start_of_year, start_of_day) {
                (true, _) => format!("{:04}/{:02}/{:02}\n{:02}:{:02}:{:02}", year, month, day, hours, minutes, seconds),
                (_, true) => format!("{:02}/{:02}\n{:02}:{:02}:{:02}", month, day, hours, minutes, seconds),
                (_, _) => format!("{:02}:{:02}:{:02}", hours, minutes, seconds),
            }
        } else {
            match (start_of_year, start_of_day) {
                (true, _) => format!("{:04}/{:02}/{:02}\n{:02}:{:02}", year, month, day, hours, minutes),
                (_, true) => format!("{:02}/{:02}\n{:02}:{:02}", month, day, hours, minutes),
                (_, _) => format!("{:02}:{:02}", hours, minutes),
            }
        }
    }
}

pub struct Timeline {
    pub created_at: Instant,
    pub terminated_at: Instant,
    pub created_timestamp: DateTime<Local>,
    pub tick: Tick,
    pub indicator: Option<Instant>,
    pub entries: Vec<TimelineEntry>,
}

pub struct TimelineEntry {
    pub avatar: Pixmap,
    pub voice_sections: Vec<VoiceSection>,
    pub streaming_sections: Vec<StreamingSection>,
    pub active_color: Color,
    pub inactive_color: Color,
    pub streaming_color: Color,
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

pub struct VoiceSection {
    pub start_ratio: f32,
    pub end_ratio: f32,
    pub fill_style: FillStyle,
}

pub struct StreamingSection {
    pub start_ratio: f32,
    pub end_ratio: f32,
}
