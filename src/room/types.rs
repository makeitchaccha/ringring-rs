use crate::room::model::History;
use chrono::TimeDelta;
use serenity::all::{ChannelId, GuildId, Timestamp, UserId, VoiceState};
use std::fmt::Display;
use tokio::time::Instant;

#[derive(Copy, Clone, Debug)]
pub struct RoomId(u64);

impl Display for RoomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}", self.0)
    }
}

pub struct RoomIdGenerator {
    next: u64,
}

impl RoomIdGenerator {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    pub fn next_id(&mut self) -> RoomId {
        let id = RoomId(self.next);
        self.next += 1;
        id
    }
}

impl Default for RoomIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct RoomLease {
    pub id: RoomId,
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub start: Moment,
    pub participants: Vec<ParticipantLease>,
}

#[derive(Clone)]
pub struct ParticipantLease {
    pub id: UserId,
    pub history: History,
}

#[derive(Debug, Clone, Copy)]
pub struct Moment {
    pub wall: Timestamp,
    pub mono: Instant,
}

impl Moment {
    pub fn new(wall: Timestamp, mono: Instant) -> Self {
        Self { wall, mono }
    }

    pub fn now() -> Self {
        Self::new(Timestamp::now(), Instant::now())
    }

    /// Computes a new `Moment` with an updated monotonic instant.
    ///
    /// Computes a new `Moment` with an updated monotonic instant.
    ///
    /// The wall-clock time is shifted by the same delta as the monotonic
    /// instant, preserving the correspondence between wall and mono time.
    /// Works in both directions: if `new_mono` is before `self.mono`, the wall
    /// clock shifts backward by the absolute delta.
    ///
    /// This is necessary for reconnect scenarios where stale Instants from a
    /// previous session are replayed into a new Room.  The wall-clock
    /// correspondence is preserved rather than saturated, so downstream
    /// consumers (e.g. rendering) can clamp as needed via
    /// `saturating_duration_since`.
    pub fn at(&self, new_mono: Instant) -> Self {
        let delta = if new_mono >= self.mono {
            TimeDelta::from_std(new_mono - self.mono).expect("Duration overflow")
        } else {
            -TimeDelta::from_std(self.mono - new_mono).expect("Duration overflow")
        };

        let new_wall = Timestamp::from(*self.wall + delta);

        Self {
            wall: new_wall,
            mono: new_mono,
        }
    }
}

#[cfg(test)]
mod moment_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn moment_at_shifts_mono() {
        let m = Moment::now();
        let new_mono = m.mono + Duration::from_secs(10);
        let m2 = m.at(new_mono);
        assert_eq!(m2.mono, new_mono);
    }

    #[test]
    fn moment_at_shifts_wall_by_delta() {
        let m = Moment::now();
        let new_mono = m.mono + Duration::from_secs(60);
        let m2 = m.at(new_mono);
        let wall_delta = *m2.wall - *m.wall;
        assert_eq!(wall_delta.num_seconds(), 60);
    }

    #[test]
    fn moment_at_zero_delta() {
        let m = Moment::now();
        let m2 = m.at(m.mono);
        assert_eq!(m2.mono, m.mono);
        assert_eq!(*m2.wall, *m.wall);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceStateFlags {
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_sharing_screen: bool,
}

impl From<&VoiceState> for VoiceStateFlags {
    fn from(state: &VoiceState) -> Self {
        VoiceStateFlags {
            is_muted: state.mute() || state.self_mute(),
            is_deafened: state.deaf() || state.self_deaf(),
            is_sharing_screen: state.self_stream().unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod voice_state_flags_tests {
    use super::*;
    use serenity::all::VoiceState;

    fn make_state(json: &str) -> VoiceState {
        serde_json::from_str(json).expect("valid voice state")
    }

    #[test]
    fn muted_via_self_mute() {
        let state = make_state(r#"{
            "channel_id": null,
            "deaf": false,
            "guild_id": null,
            "member": null,
            "mute": false,
            "self_deaf": false,
            "self_mute": true,
            "self_stream": null,
            "self_video": false,
            "session_id": "",
            "suppress": false,
            "user_id": "1",
            "request_to_speak_timestamp": null
        }"#);
        let flags = VoiceStateFlags::from(&state);
        assert!(flags.is_muted);
    }

    #[test]
    fn deafened_via_deaf() {
        let state = make_state(r#"{
            "channel_id": null,
            "deaf": true,
            "guild_id": null,
            "member": null,
            "mute": false,
            "self_deaf": false,
            "self_mute": false,
            "self_stream": null,
            "self_video": false,
            "session_id": "",
            "suppress": false,
            "user_id": "1",
            "request_to_speak_timestamp": null
        }"#);
        let flags = VoiceStateFlags::from(&state);
        assert!(flags.is_deafened);
    }

    #[test]
    fn self_stream_true_sharing() {
        let state = make_state(r#"{
            "channel_id": null,
            "deaf": false,
            "guild_id": null,
            "member": null,
            "mute": false,
            "self_deaf": false,
            "self_mute": false,
            "self_stream": true,
            "self_video": false,
            "session_id": "",
            "suppress": false,
            "user_id": "1",
            "request_to_speak_timestamp": null
        }"#);
        let flags = VoiceStateFlags::from(&state);
        assert!(flags.is_sharing_screen);
    }

    #[test]
    fn self_stream_none_not_sharing() {
        let state = make_state(r#"{
            "channel_id": null,
            "deaf": false,
            "guild_id": null,
            "member": null,
            "mute": false,
            "self_deaf": false,
            "self_mute": false,
            "self_stream": null,
            "self_video": false,
            "session_id": "",
            "suppress": false,
            "user_id": "1",
            "request_to_speak_timestamp": null
        }"#);
        let flags = VoiceStateFlags::from(&state);
        assert!(!flags.is_sharing_screen);
    }
}