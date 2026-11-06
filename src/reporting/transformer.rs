use crate::graphics::timeline::view::{AxisConfig, RatioSpan};
use crate::graphics::{
    FillStyle, MajorTickConfig, StreamingSection, Timeline, TimelineEntry, VoiceSection,
};
use crate::infrastructure::MemberVisual;
use crate::reporting::RoomSnapshot;
use crate::room::{AudioActivity, Interval};
use chrono_tz::Tz;
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
    timezone: Tz,
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
                .cloned()
                .unwrap_or_else(|| MemberVisual::fallback(p.id));

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
        created_timestamp: room.start.wall.with_timezone(&timezone).naive_local(),
        entries,
        axis: choose_suitable_axis(to.saturating_duration_since(from)),
    }
}

pub fn calculate_timeline_end(start: Instant, end: Instant) -> Instant {
    // always be fill 80% of timeline in ongoing call
    let elapsed = end.duration_since(start);

    start + max(elapsed * 10 / 8, Duration::from_secs(5))
}

pub fn choose_suitable_axis(duration: Duration) -> AxisConfig {
    const AXIS_CONFIG_PRESET: [(u64, &[u32]); 11] = [
        (604800, &[7, 1]),
        (86400, &[4, 2, 1]),
        (43200, &[6, 2]),
        (21600, &[6, 3]),
        (10800, &[3]),
        (3600, &[6, 2, 1]),
        (1800, &[6, 3]),
        (900, &[3]),
        (600, &[2]),
        (300, &[5, 1]),
        (60, &[4, 2, 1]),
    ];

    let duration_secs = duration.as_secs();

    for (interval_sec, divisions_preset) in AXIS_CONFIG_PRESET {
        let majors = duration_secs / interval_sec;
        if majors > 1 {
            let interval = Duration::from_secs(interval_sec);
            for divisions in divisions_preset {
                if majors * (*divisions as u64) < 10 {
                    return AxisConfig::with_minor(
                        MajorTickConfig::without_sec(interval),
                        *divisions,
                    )
                    .unwrap();
                }
            }
            return AxisConfig::with_minor(
                MajorTickConfig::without_sec(interval),
                divisions_preset.last().cloned().unwrap_or(1),
            )
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
    if end <= start {
        return Vec::new();
    }
    let duration = end - start;
    if duration.is_zero() {
        return Vec::new();
    }
    let duration_sec = duration.as_secs_f32();

    history
        .into_iter()
        .filter_map(|activity| {
            let fill_style = FillStyle::from_flags(activity.muted, activity.deafened);

            let start_ratio = activity.interval.start.saturating_duration_since(start).as_secs_f32() / duration_sec;
            let end_ratio =
                activity.interval.end.unwrap_or(now).saturating_duration_since(start).as_secs_f32() / duration_sec;

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
    if end <= start {
        return Vec::new();
    }
    let duration = end - start;
    if duration.is_zero() {
        return Vec::new();
    }
    let duration_sec = duration.as_secs_f32();

    activities
        .into_iter()
        .filter_map(|activity| {
            let start_ratio = activity.start.saturating_duration_since(start).as_secs_f32() / duration_sec;
            let end_ratio = activity.end.unwrap_or(now).saturating_duration_since(start).as_secs_f32() / duration_sec;

            Some(StreamingSection {
                span: RatioSpan::clamped(start_ratio, end_ratio)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use crate::reporting::{HistorySnapshot, ParticipantSnapshot, RoomSnapshot};
    use crate::room::{AudioActivity, Interval, Moment};
    use chrono_tz::Tz;
    use serenity::all::{ChannelId, GuildId, Timestamp, UserId};
    use std::collections::HashMap;
    use tokio::time::Instant;

    #[test]
    fn transform_empty_participants() {
        let base = Instant::now();
        let start = Moment::new(Timestamp::now(), base);
        let room = RoomSnapshot {
            start,
            guild_id: GuildId::new(1),
            channel_id: ChannelId::new(1),
            participants: vec![],
        };
        let from = base;
        let to = base + std::time::Duration::from_secs(10);
        let now = base;
        let visuals = HashMap::new();
        let tz: Tz = "UTC".parse().unwrap();
        let timeline = transform(from, to, now, &room, &visuals, tz);
        assert!(timeline.entries.is_empty());
    }

    #[test]
    fn transform_filters_outside_range() {
        let base = Instant::now();
        let start = Moment::new(Timestamp::now(), base);
        // activity entirely after [from,to]
        let from = base;
        let to = base + std::time::Duration::from_secs(10);
        let act_start = base + std::time::Duration::from_secs(20);
        let act_end = base + std::time::Duration::from_secs(30);
        let participant = ParticipantSnapshot {
            id: UserId::new(1),
            history: HistorySnapshot {
                audio: vec![AudioActivity {
                    interval: Interval {
                        start: act_start,
                        end: Some(act_end),
                    },
                    muted: false,
                    deafened: false,
                }],
                screen_sharing: vec![],
            },
        };
        let room = RoomSnapshot {
            start,
            guild_id: GuildId::new(1),
            channel_id: ChannelId::new(1),
            participants: vec![participant],
        };
        let now = base;
        let visuals = HashMap::new();
        let tz: Tz = "UTC".parse().unwrap();
        let timeline = transform(from, to, now, &room, &visuals, tz);
        assert!(timeline.entries.is_empty());
    }

    #[test]
    fn transform_includes_overlapping_activity() {
        let base = Instant::now();
        let start = Moment::new(Timestamp::now(), base);
        let from = base;
        let to = base + std::time::Duration::from_secs(10);
        let act_start = base + std::time::Duration::from_secs(5);
        let act_end = base + std::time::Duration::from_secs(15);
        let participant = ParticipantSnapshot {
            id: UserId::new(1),
            history: HistorySnapshot {
                audio: vec![AudioActivity {
                    interval: Interval {
                        start: act_start,
                        end: Some(act_end),
                    },
                    muted: false,
                    deafened: false,
                }],
                screen_sharing: vec![],
            },
        };
        let room = RoomSnapshot {
            start,
            guild_id: GuildId::new(1),
            channel_id: ChannelId::new(1),
            participants: vec![participant],
        };
        let now = base + std::time::Duration::from_secs(15);
        let visuals = HashMap::new();
        let tz: Tz = "UTC".parse().unwrap();
        let timeline = transform(from, to, now, &room, &visuals, tz);
        assert_eq!(timeline.entries.len(), 1);
        assert!(!timeline.entries[0].voice_sections.is_empty());
    }
}

#[cfg(test)]
mod axis_tests {
    use super::*;
    use crate::graphics::timeline::view::TickKind;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use std::time::Duration;

    fn make_dt() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
    }

    #[test]
    fn choose_suitable_axis_short_duration() {
        let axis = choose_suitable_axis(Duration::from_secs(60));
        let ticks = axis.generate_tick(make_dt(), make_dt() + chrono::TimeDelta::seconds(60));
        assert!(!ticks.is_empty(), "60s duration should produce ticks");
    }

    #[test]
    fn choose_suitable_axis_hour_duration() {
        let axis = choose_suitable_axis(Duration::from_secs(3600));
        let ticks = axis.generate_tick(make_dt(), make_dt() + chrono::TimeDelta::seconds(3600));
        assert!(!ticks.is_empty(), "1h duration should produce ticks");
        let majors = ticks.iter().filter(|t| matches!(t.kind, TickKind::Major { .. })).count();
        assert!(majors >= 2, "1h should have at least 2 major ticks, got {}", majors);
    }

    #[test]
    fn choose_suitable_axis_day_duration() {
        let axis = choose_suitable_axis(Duration::from_secs(86400));
        let ticks = axis.generate_tick(make_dt(), make_dt() + chrono::TimeDelta::seconds(86400));
        assert!(!ticks.is_empty(), "1d duration should produce ticks");
    }

    #[test]
    fn choose_suitable_axis_zero_duration() {
        let axis = choose_suitable_axis(Duration::from_secs(0));
        let ticks = axis.generate_tick(make_dt(), make_dt());
        // Should not panic
        let _ = ticks;
    }

    #[test]
    fn calculate_timeline_end_extends() {
        use tokio::time::Instant;
        let start = Instant::now();
        let end = calculate_timeline_end(start, start + std::time::Duration::from_secs(10));
        assert!(end > start + std::time::Duration::from_secs(10),
            "timeline end should extend beyond elapsed");
    }

    #[test]
    fn calculate_timeline_end_minimum() {
        use tokio::time::Instant;
        let start = Instant::now();
        let end = calculate_timeline_end(start, start);
        assert!(end >= start + std::time::Duration::from_secs(5),
            "timeline end should be at least 5s");
    }
}
