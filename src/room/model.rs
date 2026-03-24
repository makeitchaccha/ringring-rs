use crate::room::types::{Moment, ParticipantLease, RoomLease, UserIdentity, VoiceStateFlags};
use serenity::all::{ChannelId, GuildId, UserId};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::Instant;
use tracing::debug;

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
    Empty,
}

#[derive(Debug)]
pub struct Room {
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub start: Moment,
    pub participants: Vec<Participant>, // retains all participant since a room was created.
}

pub type RoomResult<T> = Result<T, RoomError>;

impl Room {
    pub fn new(guild_id: GuildId, channel_id: ChannelId, start: Moment) -> Self {
        Room {
            guild_id,
            channel_id,
            start,
            participants: Vec::new(),
        }
    }

    fn find_participant_mut(&mut self, user_id: UserId) -> Option<&mut Participant> {
        self.participants
            .iter_mut()
            .find(|part| part.identification.user_id == user_id)
    }

    pub fn handle_connect(
        &mut self,
        now: Instant,
        identification: UserIdentity,
        flags: VoiceStateFlags,
    ) -> RoomResult<()> {
        debug!("handle connect");
        if let Some(participant) = self.find_participant_mut(identification.user_id) {
            debug!("participant already exists");
            participant.connect(now, flags)?;
            return Ok(());
        }

        debug!("newly connected, create participant");
        let mut participant = Participant::new(identification);
        participant.connect(now, flags)?;
        self.participants.push(participant);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.get_status() == RoomStatus::Empty
    }

    fn get_status(&self) -> RoomStatus {
        if self.participants.iter().any(|part| part.is_connected()) {
            RoomStatus::Occupied
        } else {
            RoomStatus::Empty
        }
    }

    pub fn handle_disconnect(&mut self, now: Instant, user_id: UserId) -> RoomResult<RoomStatus> {
        debug!("handle disconnect");
        let participant = self
            .find_participant_mut(user_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.disconnect(now)?;
        let status = self.get_status();
        if status == RoomStatus::Empty {
            debug!("no one is in room");
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

    pub fn lease(&self) -> RoomLease {
        let participants = self
            .participants
            .iter()
            .map(|participant| participant.lease())
            .collect();

        RoomLease {
            channel_id: self.channel_id,
            guild_id: self.guild_id,
            start: self.start,
            participants,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub identification: UserIdentity,
    pub history: Arc<Vec<Activity>>,
}

impl Participant {
    pub fn new(identification: UserIdentity) -> Self {
        Participant {
            identification,
            history: Arc::new(Vec::new()),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.history.last().is_some_and(|a| a.is_ongoing())
    }

    pub fn connect(&mut self, now: Instant, flags: VoiceStateFlags) -> ActivityResult<()> {
        if self.is_connected() {
            return Err(ActivityError::AlreadyStarted);
        }
        let activity = Activity::start_at(now, flags);
        Arc::make_mut(&mut self.history).push(activity);
        Ok(())
    }

    pub fn disconnect(&mut self, now: Instant) -> ActivityResult<()> {
        let last = Arc::make_mut(&mut self.history)
            .last_mut()
            .ok_or(ActivityError::NoActiveActivity)?;
        last.end_at(now)?;
        Ok(())
    }

    pub fn update(&mut self, now: Instant, flags: VoiceStateFlags) -> Result<(), ActivityError> {
        if !self.is_connected() {
            return Err(ActivityError::NoActiveActivity);
        }

        let last = Arc::make_mut(&mut self.history)
            .last_mut()
            .expect("is_connected() check failed; this should not happen");
        if last.flags() == flags {
            return Ok(());
        }

        last.end_at(now)?;
        let activity = Activity::start_at(now, flags);
        Arc::make_mut(&mut self.history).push(activity);
        Ok(())
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        self.history
            .iter()
            .map(|activity| activity.calculate_duration(now))
            .sum()
    }

    pub fn lease(&self) -> ParticipantLease {
        ParticipantLease {
            identification: self.identification.clone(),
            history: self.history.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ActivityError {
    #[error("Activity has already started")]
    AlreadyStarted,

    #[error("Activity has already ended")]
    AlreadyEnded,

    #[error("No activity found")]
    NoActiveActivity,
}

pub type ActivityResult<T> = Result<T, ActivityError>;

#[derive(Debug, Clone)]
pub struct Activity {
    start: Instant,
    end: Option<Instant>,
    flags: VoiceStateFlags,
}

impl Activity {
    pub fn start_at(start: Instant, flags: VoiceStateFlags) -> Self {
        Activity {
            start,
            end: None,
            flags,
        }
    }

    pub fn end_at(&mut self, now: Instant) -> ActivityResult<()> {
        match self.end {
            Some(_) => Err(ActivityError::AlreadyEnded),
            None => {
                self.end = Some(now);
                Ok(())
            }
        }
    }

    pub fn is_ended(&self) -> bool {
        self.end.is_some()
    }

    pub fn is_ongoing(&self) -> bool {
        self.end.is_none()
    }

    pub fn is_following(&self, prev: &Activity) -> bool {
        prev.end == Some(self.start)
    }

    pub fn start(&self) -> Instant {
        self.start
    }

    pub fn end(&self) -> Option<Instant> {
        self.end
    }

    pub fn flags(&self) -> VoiceStateFlags {
        self.flags
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        if let Some(end) = self.end {
            end.duration_since(self.start)
        } else {
            now.duration_since(self.start)
        }
    }
}
