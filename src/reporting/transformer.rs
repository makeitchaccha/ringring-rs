use crate::graphics::{FillStyle, StreamingSection, Tick, Timeline, TimelineEntry, VoiceSection};
use crate::infrastructure::MemberVisual;
use crate::reporting::RoomSnapshot;
use crate::room::Activity;
use chrono::Local;
use serenity::all::UserId;
use std::collections::HashMap;
use std::ops::Add;
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
        .filter(|p| p.history.iter().any(|activity| activity.overlaps(from, to)))
        .map(|p| {
            let visual = visuals
                .get(&p.identity.user_id)
                .expect("visual must be pre-fetched before rendering.");

            TimelineEntry {
                avatar: visual.avatar.clone(),
                voice_sections: convert_to_voice_sections(from, now, to, p.history.as_ref()),
                streaming_sections: convert_to_streaming_sections(
                    from,
                    now,
                    to,
                    p.history.as_ref(),
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
        created_timestamp: room.start.wall.with_timezone(&Local),
        entries,
        tick: choose_suitable_tics(to - from),
    }
}

pub fn calculate_auto_scale(start: Instant, end: Instant) -> Instant {
    const FRAMES: [Duration; 12] = [
        Duration::from_mins(1),
        Duration::from_mins(5),
        Duration::from_mins(10),
        Duration::from_mins(30),
        Duration::from_hours(1),
        Duration::from_hours(2),
        Duration::from_hours(3),
        Duration::from_hours(4),
        Duration::from_hours(6),
        Duration::from_hours(8),
        Duration::from_hours(12),
        Duration::from_hours(24),
    ];

    let duration = end - start;

    for frame in FRAMES {
        if duration < frame {
            return start.add(frame);
        }
    }

    let duration_days = duration.as_secs() / (24 * 60 * 60);

    start.add(Duration::from_hours(24 * (1 + duration_days)))
}

fn choose_suitable_tics(duration: Duration) -> Tick {
    const TICKS: [Tick; 12] = [
        Tick::secs_grain(10),
        Tick::mins_grain(1),
        Tick::mins_grain(2),
        Tick::mins_grain(5),
        Tick::mins_grain(10),
        Tick::mins_grain(15),
        Tick::mins_grain(30),
        Tick::hours_grain(1),
        Tick::hours_grain(2),
        Tick::hours_grain(4),
        Tick::hours_grain(6),
        Tick::hours_grain(12),
    ];

    let duration_secs = duration.as_secs();

    for tick in TICKS {
        if duration_secs / tick.interval.as_secs() < 10 {
            return tick;
        }
    }

    Tick::hours_grain(24)
}

fn convert_to_voice_sections(
    start: Instant,
    now: Instant,
    end: Instant,
    history: &[Activity],
) -> Vec<VoiceSection> {
    let duration_sec = (end - start).as_secs_f32();

    history
        .iter()
        .filter(|activity| activity.overlaps(start, end))
        .map(|activity| {
            let fill_style = FillStyle::from_flags(activity.flags());

            let start_ratio = (activity.start() - start).as_secs_f32() / duration_sec;
            let end_ratio = (activity.end().unwrap_or(now) - start).as_secs_f32() / duration_sec;

            VoiceSection {
                start_ratio,
                end_ratio,
                fill_style,
            }
        })
        .collect()
}

fn convert_to_streaming_sections(
    start: Instant,
    now: Instant,
    end: Instant,
    history: &[Activity],
) -> Vec<StreamingSection> {
    let duration_sec = (end - start).as_secs_f32();
    let mut streaming_sections = Vec::new();

    // Always keep streaming start activity.
    let mut streaming_start_activity: Option<&Activity> = None;

    let history: Vec<&Activity> = history
        .iter()
        .filter(|activity| activity.overlaps(start, end))
        .collect();

    for i in 0..history.len() {
        let current_activity = &history[i];

        // update current streaming start
        match streaming_start_activity {
            Some(streaming_start) => {
                if !current_activity.flags().is_sharing_screen {
                    let start_ratio =
                        (streaming_start.start() - start).as_secs_f32() / duration_sec;
                    let end_ratio = (current_activity.start() - start).as_secs_f32() / duration_sec;

                    streaming_sections.push(StreamingSection {
                        start_ratio,
                        end_ratio,
                    });

                    streaming_start_activity = None;
                }
            }
            None => {
                if current_activity.flags().is_sharing_screen {
                    streaming_start_activity = Some(history[i]);
                }
            }
        }

        // detect disconnection from voice channel
        if let Some(streaming_start) = &streaming_start_activity {
            let terminated = if i == history.len() - 1 {
                true
            } else {
                !history[i + 1].is_following(current_activity)
            };

            if terminated {
                let start_ratio = (streaming_start.start() - start).as_secs_f32() / duration_sec;
                let end_ratio =
                    (current_activity.end().unwrap_or(now) - start).as_secs_f32() / duration_sec;

                streaming_sections.push(StreamingSection {
                    start_ratio,
                    end_ratio,
                });
                streaming_start_activity = None;
            }
        }
    }

    streaming_sections
}
