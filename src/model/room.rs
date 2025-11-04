use std::time::SystemTime;
use serenity::all::{ChannelId, GuildId, UserId};
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
    participants: Vec<Participant>, // retains all participant since a room was created.
}

pub type RoomResult<T> = Result<T, RoomError>;

impl From<ActivityError> for RoomError {
    fn from(err: ActivityError) -> Self {
        RoomError::Activity(err)
    }
}

impl Room {
    pub fn new(guild_id: GuildId, channel_id: ChannelId, start: SystemTime) -> Self {
        Room {
            guild_id,
            channel_id,
            start,
            participants: Vec::new(),
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

    pub fn handle_connect(&mut self, now: SystemTime, user_id: UserId, name: String, flags: VoiceStateFlags) -> RoomResult<()> {
        if let Some(participant) = self.find_participant_mut(user_id) {
            participant.connect(now, flags)?;
            return Ok(())
        }

        let mut participant = Participant::new(user_id, name);
        participant.connect(now, flags)?;
        self.participants.push(participant);
        Ok(())
    }

    fn get_status(&self) -> RoomStatus {
        if self.participants.iter().any(|part| part.is_connected()) {
            RoomStatus::Occupied
        } else {
            RoomStatus::Idle
        }
    }

    pub fn handle_disconnect(&mut self, now: SystemTime, user_id: UserId) -> RoomResult<RoomStatus> {
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.disconnect(now)?;
        Ok(self.get_status())
    }

    pub fn handle_update(&mut self, now: SystemTime, user_id: UserId, flags: VoiceStateFlags) -> RoomResult<()> {
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.update(now, flags)?;
        Ok(())
    }
}
