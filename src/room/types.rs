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

    pub fn at(&self, new_mono: Instant) -> Self {
        let wall = self.wall.with_timezone(&chrono::Local);
        let delta = TimeDelta::from_std(new_mono - self.mono).expect("Duration overflow");

        let new_wall = Timestamp::from(wall + delta);

        Self {
            wall: new_wall,
            mono: new_mono,
        }
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
