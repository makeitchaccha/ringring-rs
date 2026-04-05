use crate::room::RoomId;
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
    ParticipantInconsistency(#[from] ParticipantError),

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
    pub id: RoomId,
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub start: Moment,
    pub participants: Vec<Participant>, // retains all participant since a room was created.
}

pub type RoomResult<T> = Result<T, RoomError>;

impl Room {
    pub fn new(id: RoomId, guild_id: GuildId, channel_id: ChannelId, start: Moment) -> Self {
        Room {
            id,
            guild_id,
            channel_id,
            start,
            participants: Vec::new(),
        }
    }

    fn find_participant_mut(&mut self, user_id: UserId) -> Option<&mut Participant> {
        self.participants
            .iter_mut()
            .find(|part| part.identity.user_id == user_id)
    }

    pub fn handle_connect(
        &mut self,
        now: Instant,
        identity: UserIdentity,
        flags: VoiceStateFlags,
    ) -> RoomResult<()> {
        debug!("handle connect");
        if let Some(participant) = self.find_participant_mut(identity.user_id) {
            debug!("participant already exists");
            participant.connect(now, flags)?;
            return Ok(());
        }

        debug!("newly connected, create participant");
        let mut participant = Participant::new(identity);
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
            id: self.id,
            channel_id: self.channel_id,
            guild_id: self.guild_id,
            start: self.start,
            participants,
        }
    }
}

#[derive(Debug, Error)]
pub enum ParticipantError {
    #[error("Invalid Participant Status")]
    InvalidState,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub identity: UserIdentity,
    pub history: History,
}

impl Participant {
    pub fn new(identity: UserIdentity) -> Self {
        Participant {
            identity,
            history: History {
                audio: Arc::new(Vec::new()),
                screen_sharing: Arc::new(Vec::new()),
            },
        }
    }

    pub fn is_connected(&self) -> bool {
        self.history
            .audio
            .last()
            .is_some_and(|a| a.interval.is_ongoing())
    }

    pub fn connect(
        &mut self,
        now: Instant,
        flags: VoiceStateFlags,
    ) -> Result<(), ParticipantError> {
        if self.is_connected() {
            return Err(ParticipantError::InvalidState);
        }
        let interval = Interval::start_at(now);
        let audio_activity = AudioActivity {
            interval,
            muted: flags.is_muted,
            deafened: flags.is_deafened,
        };
        let activities = Arc::make_mut(&mut self.history.audio);
        activities.push(audio_activity);

        if flags.is_sharing_screen {
            let interval = Interval::start_at(now);
            let activities = Arc::make_mut(&mut self.history.screen_sharing);
            activities.push(interval);
        }
        Ok(())
    }

    pub fn disconnect(&mut self, now: Instant) -> Result<(), ParticipantError> {
        let last_audio = Arc::make_mut(&mut self.history.audio)
            .last_mut()
            .ok_or(ParticipantError::InvalidState)?;
        last_audio
            .interval
            .end_at(now)
            .or(Err(ParticipantError::InvalidState))?;

        let Some(last_screen_sharing) = Arc::make_mut(&mut self.history.screen_sharing).last_mut()
        else {
            return Ok(());
        };
        if last_screen_sharing.is_ongoing() {
            last_screen_sharing
                .end_at(now)
                .or(Err(ParticipantError::InvalidState))?;
        }
        Ok(())
    }

    pub fn update(&mut self, now: Instant, flags: VoiceStateFlags) -> Result<(), ParticipantError> {
        self.update_audio(now, flags)?;
        self.update_screen_sharing(now, flags)?;
        Ok(())
    }

    fn update_audio(&mut self, now: Instant, flags: VoiceStateFlags) -> Result<(), ParticipantError> {
        if !self.is_connected() {
            return Err(ParticipantError::InvalidState);
        }

        let activities = Arc::make_mut(&mut self.history.audio);
        let last_activity = activities
            .last_mut()
            .expect("is_connected() check failed; this should not happen");
        if last_activity.is_same_state(flags) {
            return Ok(())
        }
        last_activity
            .interval
            .end_at(now)
            .or(Err(ParticipantError::InvalidState))?;
        let activity = AudioActivity {
            interval: Interval::start_at(now),
            muted: flags.is_muted,
            deafened: flags.is_deafened,
        };
        activities.push(activity);
        Ok(())
    }

    fn update_screen_sharing(&mut self, now: Instant, flags: VoiceStateFlags) -> Result<(), ParticipantError> {
        let activities = Arc::make_mut(&mut self.history.screen_sharing);
        let Some(last_activity) = activities.last_mut() else {
            if flags.is_sharing_screen {
                activities.push(Interval::start_at(now));
            }
            return Ok(());
        };

        match (last_activity.is_ongoing(), flags.is_sharing_screen) {
            (false, true) => {
                activities.push(Interval::start_at(now));
            }
            (true, false) => {
                last_activity
                    .end_at(now)
                    .or(Err(ParticipantError::InvalidState))?;
            }
            (_, _) => {}
        }
        Ok(())
    }

    pub fn lease(&self) -> ParticipantLease {
        ParticipantLease {
            identity: self.identity.clone(),
            history: self.history.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct History {
    pub audio: Arc<Vec<AudioActivity>>,
    pub screen_sharing: Arc<Vec<Interval>>,
}

#[derive(Debug)]
pub enum IntervalError {
    AlreadyEnded,
    NoActiveActivity,
}

pub type IntervalResult<T> = Result<T, IntervalError>;

#[derive(Debug, Clone)]
pub struct AudioActivity {
    pub interval: Interval,
    pub muted: bool,
    pub deafened: bool,
}

impl AudioActivity {
    fn is_same_state(&self, flags: VoiceStateFlags) -> bool {
        self.muted == flags.is_muted && self.deafened == flags.is_deafened
    }
}

#[derive(Debug, Clone)]
pub struct Interval {
    pub start: Instant,
    pub end: Option<Instant>,
}

impl Interval {
    pub fn start_at(start: Instant) -> Self {
        Interval { start, end: None }
    }

    pub fn end_at(&mut self, now: Instant) -> IntervalResult<()> {
        match self.end {
            Some(_) => Err(IntervalError::AlreadyEnded),
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

    pub fn is_following(&self, prev: &Interval) -> bool {
        prev.end == Some(self.start)
    }

    pub fn overlaps(&self, range_start: Instant, range_end: Instant) -> bool {
        self.start <= range_end && self.end.is_none_or(|end| range_start <= end)
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        if let Some(end) = self.end {
            end.duration_since(self.start)
        } else {
            now.duration_since(self.start)
        }
    }
}
