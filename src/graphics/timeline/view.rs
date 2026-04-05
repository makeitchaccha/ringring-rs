use chrono::{Datelike, DurationRound, NaiveDateTime, TimeDelta, Timelike};
use std::fmt::Debug;
use std::time::Duration;
use tiny_skia::{Color, Pixmap};
use tokio::time::Instant;

pub struct AxisConfig {
    major: MajorTickConfig,
    minor_divisions: Option<u32>,
}

impl AxisConfig {
    pub fn only_major(major: MajorTickConfig) -> Self {
        Self {
            major,
            minor_divisions: None,
        }
    }

    pub const fn with_minor(
        major: MajorTickConfig,
        minor_divisions: u32,
    ) -> Result<Self, &'static str> {
        if minor_divisions == 0 {
            return Err("Divisions must be non-zero");
        }
        Ok(Self {
            major,
            minor_divisions: Some(minor_divisions),
        })
    }

    pub fn generate_tick(&self, start: NaiveDateTime, end: NaiveDateTime) -> Vec<Tick> {
        let step = TimeDelta::from_std(self.major.interval).unwrap();
        let divisions = self.minor_divisions.unwrap_or(1) as i32;

        let initial = start.duration_trunc(step).unwrap();

        let elapsed = end - start;

        let mut ticks = Vec::new();
        for i in 0.. {
            let current = initial + step * i / divisions;
            if current < start {
                continue;
            }
            if current > end {
                break;
            }

            let ratio = (current - start).as_seconds_f32() / elapsed.as_seconds_f32();
            if i % divisions == 0 {
                ticks.push(Tick {
                    ratio,
                    kind: TickKind::Major {
                        format: self.major.determine_time_format(current),
                        timestamp: current,
                    },
                })
            } else {
                ticks.push(Tick {
                    ratio,
                    kind: TickKind::Minor,
                })
            }
        }

        ticks
    }
}

pub struct Tick {
    pub ratio: f32,
    pub kind: TickKind,
}

pub enum TickKind {
    Major {
        format: &'static [&'static str],
        timestamp: NaiveDateTime,
    },
    Minor,
}

#[derive(Debug, Copy, Clone)]
pub struct MajorTickConfig {
    pub interval: Duration,
    pub format_normal: &'static [&'static str],
    pub format_date: Option<&'static [&'static str]>,
    pub format_year: Option<&'static [&'static str]>,
}

impl MajorTickConfig {
    pub const fn with_sec(interval: Duration) -> Self {
        Self {
            interval,
            format_normal: &["%H:%M:%S"],
            format_date: Some(&["%H:%M:%S", "%m/%d"]),
            format_year: Some(&["%H:%M:%S", "%Y/%m/%d"]),
        }
    }

    pub const fn without_sec(interval: Duration) -> Self {
        Self {
            interval,
            format_normal: &["%H:%M"],
            format_date: Some(&["%H:%M", "%m/%d"]),
            format_year: Some(&["%H:%M", "%Y/%m/%d"]),
        }
    }

    pub fn determine_time_format(&self, time: NaiveDateTime) -> &'static [&'static str] {
        let start_of_day = time.hour() == 0 && time.minute() == 0;
        let start_of_year = start_of_day && time.month() == 1 && time.day() == 1;

        if start_of_year && let Some(format_year) = self.format_year {
            return format_year;
        }

        if start_of_day && let Some(format_date) = self.format_date {
            return format_date;
        }

        self.format_normal
    }
}

pub struct Timeline {
    pub created_at: Instant,
    pub terminated_at: Instant,
    pub created_timestamp: NaiveDateTime,
    pub axis: AxisConfig,
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
    pub fn from_flags(muted: bool, deafened: bool) -> FillStyle {
        match (deafened, muted) {
            (true, _) => FillStyle::Deafened,
            (_, true) => FillStyle::Muted,
            (_, _) => FillStyle::Active,
        }
    }
}

pub struct RatioSpan {
    start: f32,
    end: f32,
}

impl RatioSpan {
    pub fn new(start_ratio: f32, end_ratio: f32) -> Option<RatioSpan> {
        if end_ratio <= start_ratio {
            return None;
        }

        Some(RatioSpan {
            start: Self::must_be_0to1(start_ratio)?,
            end: Self::must_be_0to1(end_ratio)?,
        })
    }

    #[inline]
    fn must_be_0to1(x: f32) -> Option<f32> {
        (0.0..=1.0).contains(&x).then_some(x)
    }

    pub fn clamped(start_ratio: f32, end_ratio: f32) -> Option<RatioSpan> {
        let clamped_start = start_ratio.max(0.0);
        let clamped_end = end_ratio.min(1.0);

        if clamped_end <= clamped_start {
            return None;
        }

        Some(RatioSpan {
            start: clamped_start,
            end: clamped_end,
        })
    }

    #[inline]
    pub fn start(&self) -> f32 {
        self.start
    }

    #[inline]
    pub fn end(&self) -> f32 {
        self.end
    }
}

pub struct VoiceSection {
    pub span: RatioSpan,
    pub fill_style: FillStyle,
}

pub struct StreamingSection {
    pub span: RatioSpan,
}
