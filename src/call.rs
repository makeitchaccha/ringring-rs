use std::collections::HashMap;
use std::time::SystemTime;
use serenity::all::{ChannelId, GuildId, UserId, VoiceState};

pub struct RoomManager{
    rooms: HashMap<ChannelId, Room>,
}

pub enum RoomError {
    ParticipantNotFound,
    Activity(ActivityError),
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
        self.participants.iter().find(|part| part.user_id == user_id)
    }

    fn find_participant_mut(&mut self, user_id: UserId) -> Option<&mut Participant> {
        self.participants.iter_mut().find(|part| part.user_id == user_id)
    }

    pub fn handle_connect(&mut self, now: SystemTime, user_id: UserId, name: String, flags: VoiceStateFlags) -> RoomResult<()> {
        if let Some(participant) = self.find_participant_mut(user_id) {
            participant.name = name;
            participant.connect(now, flags)?;
            return Ok(())
        }

        let mut participant = Participant::new(user_id, name);
        participant.connect(now, flags)?;
        self.participants.push(participant);
        Ok(())
    }

    pub fn handle_disconnect(&mut self, now: SystemTime, user_id: UserId) -> RoomResult<RoomStatus> {
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.disconnect(now)?;
        Ok(())
    }

    pub fn handle_update(&mut self, now: SystemTime, user_id: UserId, flags: VoiceStateFlags) -> RoomResult<()> {
        let participant = self.find_participant_mut(user_id).ok_or(RoomError::ParticipantNotFound)?;
        participant.update(now, flags)?;
        Ok(())
    }
}

pub struct Participant{
    user_id: UserId,
    name: String,
    history: Vec<Activity>
}

impl Participant {
    fn new(user_id: UserId, name: String) -> Self {
        Participant{
            user_id,
            name,
            history: Vec::new(),
        }
    }

    fn is_currently_active(&self) -> bool {
        self.history.last().map_or(false, |a| a.end.is_none())
    }

    fn connect(&mut self, now: SystemTime, flags: VoiceStateFlags) -> Result<(), ActivityError> {
        if self.is_currently_active() {
            return Err(ActivityError::AlreadyStarted)
        }
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }

    fn disconnect(&mut self, now: SystemTime) -> Result<(), ActivityError> {
        let last = self.history.last_mut().ok_or(ActivityError::NoActiveActivity)?;
        last.end_at(now)?;
        Ok(())
    }

    fn update(&mut self, now: SystemTime, flags: VoiceStateFlags) -> Result<(), ActivityError> {
        let last = self.history.last_mut().ok_or(ActivityError::NoActiveActivity)?;
        last.end_at(now)?;
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }
}

#[derive(Debug)]
pub enum ActivityError {
    AlreadyStarted,
    AlreadyEnded,
    NoActiveActivity,
}

pub type ActivityResult<T> = Result<T, ActivityError>;

pub struct Activity {
    start: SystemTime,
    end: Option<SystemTime>,
    flags: VoiceStateFlags
}

impl Activity {
    fn start_at(start: SystemTime, flags: VoiceStateFlags) -> Self {
        Activity{
            start,
            end: None,
            flags,
        }
    }

    fn end_at(&mut self, now: SystemTime) -> ActivityResult<()> {
        match self.end {
            Some(end) => Err(ActivityError::AlreadyEnded),
            None => {
                self.end = Some(now);
                Ok(())
            }
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
            is_muted: state.mute || state.self_mute,
            is_deafened: state.deaf || state.self_deaf,
            is_sharing_screen: state.self_stream.unwrap_or(false)
        }
    }
}