use crate::graphics::timeline::view::{AxisConfig, RatioSpan};
use crate::graphics::{
    FillStyle, MajorTickConfig, StreamingSection, Timeline, TimelineEntry, VoiceSection,
};
use crate::infrastructure::MemberVisual;
use crate::reporting::RoomSnapshot;
use crate::room::{AudioActivity, Interval};
use chrono::Local;
use serenity::all::UserId;
use std::cmp::max;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

pub fn transform(
    from: Instant,
    to: Instant,
    now: Instant,
    room: &RoomSnapshot,
    visuals: &HashMap<UserId, MemberVisual>,
) -> Timeline {
    let entries = room
        .participants
        .iter()
        .filter(|p| {
            p.history
                .audio
                .iter()
                .any(|activity| activity.interval.overlaps(from, to))
        })
        .map(|p| {
            let visual = visuals
                .get(&p.id)
                .expect("visual must be pre-fetched before rendering.");

            let filtered_audio_activities = p
                .history
                .audio
                .iter()
                .filter(|activity| activity.interval.overlaps(from, to));

            let filtered_screen_sharing_activities = p
                .history
                .screen_sharing
                .iter()
                .filter(|activity| activity.overlaps(from, to));

            TimelineEntry {
                avatar: visual.avatar.clone(),
                voice_sections: convert_to_voice_sections(
                    from,
                    now,
                    to,
                    filtered_audio_activities.clone(),
                ),
                streaming_sections: convert_to_streaming_sections(
                    from,
                    now,
                    to,
                    filtered_screen_sharing_activities,
                ),
                active_color: visual.active_color,
                streaming_color: visual.streaming_color,
                inactive_color: visual.inactive_color,
            }
        })
        .collect();

    Timeline {
        created_at: from,
        terminated_at: to,
        created_timestamp: room.start.wall.with_timezone(&Local).naive_local(),
        entries,
        axis: choose_suitable_axis(to - from),
    }
}

pub fn calculate_auto_scale(start: Instant, end: Instant) -> Instant {
    // always be fill 80% of timeline in ongoing call
    let elapsed = end.duration_since(start);

    start + max(elapsed * 10 / 8, Duration::from_secs(5))
}

fn choose_suitable_axis(duration: Duration) -> AxisConfig {
    const AXIS_CONFIG_PRESET: [(u64, u32); 11] = [
        (604800, 7),
        (86400, 8),
        (43200, 6),
        (21600, 6),
        (10800, 3),
        (3600, 6),
        (1800, 6),
        (900, 3),
        (600, 2),
        (300, 5),
        (60, 4),
    ];

    let duration_secs = duration.as_secs();

    for (interval_sec, divisions) in AXIS_CONFIG_PRESET {
        if duration_secs / interval_sec > 1 {
            let interval = Duration::from_secs(interval_sec);
            return AxisConfig::with_minor(MajorTickConfig::without_sec(interval), divisions)
                .unwrap();
        }
    }

    AxisConfig::with_minor(MajorTickConfig::with_sec(Duration::from_secs(15)), 3).unwrap()
}

fn convert_to_voice_sections<'a>(
    start: Instant,
    now: Instant,
    end: Instant,
    history: impl IntoIterator<Item = &'a AudioActivity>,
) -> Vec<VoiceSection> {
    let duration_sec = (end - start).as_secs_f32();

    history
        .into_iter()
        .filter_map(|activity| {
            let fill_style = FillStyle::from_flags(activity.muted, activity.deafened);

            let start_ratio = (activity.interval.start - start).as_secs_f32() / duration_sec;
            let end_ratio =
                (activity.interval.end.unwrap_or(now) - start).as_secs_f32() / duration_sec;

            Some(VoiceSection {
                span: RatioSpan::clamped(start_ratio, end_ratio)?,
                fill_style,
            })
        })
        .collect()
}

fn convert_to_streaming_sections<'a>(
    start: Instant,
    now: Instant,
    end: Instant,
    activities: impl IntoIterator<Item = &'a Interval>,
) -> Vec<StreamingSection> {
    let duration_sec = (end - start).as_secs_f32();

    activities
        .into_iter()
        .filter_map(|activity| {
            let start_ratio = (activity.start - start).as_secs_f32() / duration_sec;
            let end_ratio = (activity.end.unwrap_or(now) - start).as_secs_f32() / duration_sec;

            Some(StreamingSection {
                span: RatioSpan::clamped(start_ratio, end_ratio)?,
            })
        })
        .collect()
}
