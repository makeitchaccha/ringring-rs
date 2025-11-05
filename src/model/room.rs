use std::time::SystemTime;
use serenity::all::{ChannelId, GuildId, UserId};
use tokio::time::Instant;
use crate::model::activity::{ActivityError, VoiceStateFlags};
use crate::model::participant::Participant;

pub enum RoomError {
    ParticipantNotFound,
    Activity(ActivityError),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RoomStatus {
    Occupied,
    Idle,
}

pub struct Room {
    guild_id: GuildId,
    channel_id: ChannelId,
    start: SystemTime,
    created_at: Instant,
    participants: Vec<Participant>, // retains all participant since a room was created.
    expires_at: Option<Instant>,
}

pub type RoomResult<T> = Result<T, RoomError>;

impl From<ActivityError> for RoomError {
    fn from(err: ActivityError) -> Self {
        RoomError::Activity(err)
    }
}

impl Room {
    pub fn new(guild_id: GuildId, channel_id: ChannelId, created_at: Instant, start: SystemTime) -> Self {
        Room {
            guild_id,
            channel_id,
            start,
            created_at,
            participants: Vec::new(),
            expires_at: None,
        }
    }

    fn guild_id(&self) -> GuildId {
        self.guild_id
    }

    fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    fn find_participant(&self, user_id: UserId) -> Option<&Participant> {
        self.participants.iter().find(|part| part.user_id() == user_id)
    }

    fn find_participant_mut(&mut self, user_id: UserId) -> Option<&mut Participant> {
        self.participants.iter_mut().find(|part| part.user_id() == user_id)
    }

    pub fn handle_connect(&mut self, now: Instant, user_id: UserId, name: String, flags: VoiceStateFlags) -> RoomResult<()> {
        if let Some(participant) = self.find_participant_mut(user_id) {
            participant.connect(now, flags)?;
            self.expires_at = None;
            return Ok(())
        }

        let mut participant = Participant::new(user_id, name);
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
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.disconnect(now)?;
        let status = self.get_status();
        if status == RoomStatus::Idle {
            // fixme: literal duration
            self.expires_at = Some(now + std::time::Duration::from_secs(60));
        }
        Ok(status)
    }

    pub fn handle_update(&mut self, now: Instant, user_id: UserId, flags: VoiceStateFlags) -> RoomResult<()> {
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.update(now, flags)?;
        Ok(())
    }

    pub fn has_expired(&self, now: Instant) -> bool {
        self.expires_at.map_or(false, |expires_at| now > expires_at)
    }
}
