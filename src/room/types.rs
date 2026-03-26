use crate::room::model::Activity;
use serenity::all::{ChannelId, GuildId, Timestamp, UserId, VoiceState};
use std::sync::Arc;
use tokio::time::Instant;

pub struct RoomLease {
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub start: Moment,
    pub participants: Vec<ParticipantLease>,
}

pub struct ParticipantLease {
    pub identity: UserIdentity,
    pub history: Arc<Vec<Activity>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Moment {
    wall: Timestamp,
    mono: Instant,
}

impl Moment {
    pub fn new(wall: Timestamp, mono: Instant) -> Self {
        Self { wall, mono }
    }

    pub fn now() -> Self {
        Self::new(Timestamp::now(), Instant::now())
    }
}

#[derive(Debug, Clone)]
pub struct UserIdentity {
    pub user_id: UserId,
    pub name: Arc<str>,
    pub face: Arc<str>,
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
            is_muted: state.mute || state.self_mute,
            is_deafened: state.deaf || state.self_deaf,
            is_sharing_screen: state.self_stream.unwrap_or(false),
        }
    }
}
