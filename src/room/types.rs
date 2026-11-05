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
    /// The wall-clock time is shifted by the same delta as the monotonic
    /// instant, preserving the correspondence between wall and mono time.
    ///
    /// # Panics
    /// Panics if `new_mono < self.mono` (Instant underflow) or if the
    /// resulting time delta exceeds ~292 million years.  In practice,
    /// `new_mono` is always a later `Instant` from the same session, so
    /// this never triggers.
    pub fn at(&self, new_mono: Instant) -> Self {
        let delta = TimeDelta::from_std(new_mono - self.mono).expect("Duration overflow");

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
