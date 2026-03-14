use crate::model::activity::{ActivityError, VoiceStateFlags};
use crate::model::participant::{Identification, Participant};
use serenity::all::{ChannelId, GuildId, Timestamp, UserId};
use thiserror::Error;
use tokio::time::Instant;
use tracing::debug;

const IDLE_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Error)]
pub enum RoomError {
    #[error("Failed to find participant")]
    ParticipantNotFound,

    #[error("Failed to process activity: {0}")]
    Activity(#[from] ActivityError),

    #[error("Room has been already disposed.")]
    AlreadyDisposed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RoomStatus {
    Occupied,
    Idle,
}

#[derive(Debug)]
pub struct Room {
    guild_id: GuildId,
    channel_id: ChannelId,
    timestamp: Timestamp,
    created_at: Instant,
    participants: Vec<Participant>, // retains all participant since a room was created.
    expires_at: Option<Instant>,
}

pub type RoomResult<T> = Result<T, RoomError>;

impl Room {
    pub fn new(
        guild_id: GuildId,
        channel_id: ChannelId,
        created_at: Instant,
        timestamp: Timestamp,
    ) -> Self {
        Room {
            guild_id,
            channel_id,
            timestamp,
            created_at,
            participants: Vec::new(),
            expires_at: None,
        }
    }

    pub fn guild_id(&self) -> GuildId {
        self.guild_id
    }

    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    pub fn participants(&self) -> &Vec<Participant> {
        self.participants.as_ref()
    }

    fn find_participant_mut(&mut self, user_id: UserId) -> Option<&mut Participant> {
        self.participants
            .iter_mut()
            .find(|part| part.identification.user_id == user_id)
    }

    pub fn handle_connect(
        &mut self,
        now: Instant,
        identification: Identification,
        flags: VoiceStateFlags,
    ) -> RoomResult<()> {
        debug!("handle connect");
        if let Some(participant) = self.find_participant_mut(identification.user_id) {
            debug!("participant already exists");
            participant.connect(now, flags)?;
            self.expires_at = None;
            return Ok(());
        }

        debug!("newly connected, create participant");
        let mut participant = Participant::new(identification);
        participant.connect(now, flags)?;
        self.participants.push(participant);
        self.expires_at = None;
        Ok(())
    }

    fn get_status(&self) -> RoomStatus {
        if self.participants.iter().any(|part| part.is_connected()) {
            RoomStatus::Occupied
        } else {
            RoomStatus::Idle
        }
    }

    pub fn handle_disconnect(&mut self, now: Instant, user_id: UserId) -> RoomResult<RoomStatus> {
        debug!("handle disconnect");
        let participant = self
            .find_participant_mut(user_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.disconnect(now)?;
        let status = self.get_status();
        if status == RoomStatus::Idle {
            debug!("no one is in room");
            self.expires_at = Some(now + std::time::Duration::from_secs(IDLE_TIMEOUT_SECS));
        }
        debug!("finish handle disconnect");
        Ok(status)
    }

    pub fn handle_update(
        &mut self,
        now: Instant,
        user_id: UserId,
        flags: VoiceStateFlags,
    ) -> RoomResult<()> {
        debug!("handle update");
        let participant = self
            .find_participant_mut(user_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.update(now, flags)?;
        debug!("finish handle update");
        Ok(())
    }

    pub fn has_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now > expires_at)
    }
}
