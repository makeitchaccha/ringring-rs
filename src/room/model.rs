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

/// Represents a voice channel session's activity state.
///
/// `Room` is the root of the domain model, tracking all participants and their
/// activities during a single session. It is designed to be independent of
/// external I/O, receiving time (`Instant`) and events from the caller.
#[derive(Debug)]
pub struct Room {
    pub id: RoomId,
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub start: Moment,
    /// The list of participants who have joined this room at any point.
    pub participants: Vec<Participant>,
}

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

    /// Handles a user connecting to the voice channel.
    pub fn handle_connect(
        &mut self,
        now: Instant,
        identity: UserIdentity,
        flags: VoiceStateFlags,
    ) -> Result<(), RoomError> {
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

    /// Returns true if there are no participants currently connected to the room.
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

    /// Handles a user disconnecting from the voice channel.
    pub fn handle_disconnect(
        &mut self,
        now: Instant,
        user_id: UserId,
    ) -> Result<RoomStatus, RoomError> {
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

    /// Handles updates to a participant's voice state (e.g., mute, deafen, or screen sharing).
    pub fn handle_update(
        &mut self,
        now: Instant,
        user_id: UserId,
        flags: VoiceStateFlags,
    ) -> Result<(), RoomError> {
        debug!("handle update");
        let participant = self
            .find_participant_mut(user_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.update(now, flags)?;
        debug!("finish handle update");
        Ok(())
    }

    /// Creates a lightweight, thread-safe lease of the current room state.
    ///
    /// This is used to share the room's state with other tasks (like the reporter)
    /// without locking the main room instance.
    ///
    /// ### Performance Note
    /// The recipient should drop the returned lease as soon as possible.
    /// Holding onto it keeps the reference count of the internal activity history high,
    /// which prevents efficient in-place updates (via Copy-on-Write) in the main room task
    /// and forces a full clone of the history vector on every subsequent update.
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

/// Represents a single user's participation and their activity history in a room.
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

    /// Returns true if the participant is currently connected (i.e., has an ongoing audio interval).
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

        // Use Arc::make_mut to implement Copy-on-Write for the history.
        // This ensures that we only clone the vector if it's being shared
        // (e.g., during a reporting snapshot).
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

    fn update_audio(
        &mut self,
        now: Instant,
        flags: VoiceStateFlags,
    ) -> Result<(), ParticipantError> {
        if !self.is_connected() {
            return Err(ParticipantError::InvalidState);
        }

        let activities = Arc::make_mut(&mut self.history.audio);
        let last_activity = activities
            .last_mut()
            .expect("is_connected() check failed; this should not happen");
        if last_activity.is_same_state(flags) {
            return Ok(());
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

    fn update_screen_sharing(
        &mut self,
        now: Instant,
        flags: VoiceStateFlags,
    ) -> Result<(), ParticipantError> {
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

/// The activity history of a participant, stored efficiently using `Arc` for sharing.
#[derive(Debug, Clone)]
pub struct History {
    /// Audio activities, including mute and deafen states.
    pub audio: Arc<Vec<AudioActivity>>,
    /// Screen sharing intervals.
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
