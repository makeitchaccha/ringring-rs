use crate::room::RoomId;
use crate::room::types::{Moment, ParticipantLease, RoomLease, VoiceStateFlags};
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

    pub fn is_connected(&self, id: UserId) -> bool {
        self.participants
            .iter()
            .find(|part| part.id == id)
            .is_some_and(|part| part.is_connected())
    }

    fn find_participant_mut(&mut self, id: UserId) -> Option<&mut Participant> {
        self.participants.iter_mut().find(|part| part.id == id)
    }

    /// Routes a voice state update to connect or update handling based on
    /// whether the participant is currently connected.
    pub fn handle_voice_state(
        &mut self,
        now: Instant,
        id: UserId,
        flags: VoiceStateFlags,
    ) {
        if self.is_connected(id) {
            let _ = self.handle_update(now, id, flags);
        } else {
            let _ = self.handle_connect(now, id, flags);
        }
    }

    fn handle_connect(
        &mut self,
        now: Instant,
        id: UserId,
        flags: VoiceStateFlags,
    ) -> Result<(), RoomError> {
        debug!("handle connect");
        if let Some(participant) = self.find_participant_mut(id) {
            debug!("participant already exists");
            participant.connect(now, flags)?;
            return Ok(());
        }

        debug!("newly connected, create participant");
        let mut participant = Participant::new(id);
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
    pub fn handle_disconnect(&mut self, now: Instant, id: UserId) -> Result<RoomStatus, RoomError> {
        debug!("handle disconnect");
        let participant = self
            .find_participant_mut(id)
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
    fn handle_update(
        &mut self,
        now: Instant,
        id: UserId,
        flags: VoiceStateFlags,
    ) -> Result<(), RoomError> {
        debug!("handle update");
        let participant = self
            .find_participant_mut(id)
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
    /// ### Copy-on-Write design
    ///
    /// `History` stores its activity vectors behind `Arc<Vec<...>>`.  `lease()`
    /// only bumps the reference count (O(1)) instead of deep-cloning the data.
    ///
    /// - **Read path (lease)**: `Arc::clone` — O(1), no data copy.
    /// - **Write path (update)**: `Arc::make_mut` — O(1) when no lease is held
    ///   (in-place mutation), O(n) only while a lease keeps the refcount > 1.
    ///
    /// The `Arc` reference is kept low by two mechanisms:
    /// 1. The Reporter converts `RoomLease` to `RoomSnapshot` (owned `Vec`)
    ///    promptly, dropping the `Arc`.
    /// 2. `Session` sends [`SessionEvent::PreUpdate`] before mutating, which
    ///    evicts the previous `Updated` from the broadcast buffer (capacity 1),
    ///    dropping its `Arc` reference before `Arc::make_mut` is called.
    ///
    /// Why not `Mutex<Vec>`?
    /// - The Room task and the Reporter task would contend on the same lock.
    /// - `std::sync::Mutex` risks deadlock in async; `tokio::sync::Mutex` adds
    ///   await overhead on every access.
    ///
    /// Why not deep-clone on every lease?
    /// - With 10 participants × 1000 events, each 5–15 s lease would copy
    ///   10 000 elements. CoW defers that cost to the rare case where a lease
    ///   is actually held during an update.
    ///
    /// Why not an immutable data structure (`im` crate)?
    /// - Adds a dependency for functionality already provided by `Arc<Vec>`
    ///   with standard-library-only code.
    ///
    /// ### Performance Note
    /// The recipient should drop the returned lease as soon as possible.
    /// While a lease is held, the first mutating update clones the history vector
    /// (O(n)) because the `Arc` refcount is > 1.  Subsequent updates mutate the
    /// clone in place (O(1)) even if the lease is still alive, but the clone
    /// increases memory usage until the lease is dropped.
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
    pub id: UserId,
    pub history: History,
}

impl Participant {
    pub fn new(id: UserId) -> Self {
        Participant {
            id,
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

        // Check state before cloning the history vector.
        let same_state = self
            .history
            .audio
            .last()
            .map(|a| a.is_same_state(flags))
            .unwrap_or(false);
        if same_state {
            return Ok(());
        }

        let activities = Arc::make_mut(&mut self.history.audio);
        let Some(last_activity) = activities.last_mut() else {
            return Err(ParticipantError::InvalidState);
        };
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
        // Determine whether this is a no-op before cloning the history vector.
        let last_ongoing = self.history.screen_sharing.last().map(|a| a.is_ongoing());
        let is_noop = matches!(
            (last_ongoing, flags.is_sharing_screen),
            (None, false) | (Some(false), false) | (Some(true), true)
        );
        if is_noop {
            return Ok(());
        }

        let activities = Arc::make_mut(&mut self.history.screen_sharing);
        let Some(last_activity) = activities.last_mut() else {
            // No activity yet but sharing is starting.
            activities.push(Interval::start_at(now));
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

    /// Creates a thread-safe snapshot of this participant's history.
    ///
    /// Only bumps the `Arc` reference count (O(1)); the underlying vectors are
    /// shared until the main task mutates them via `Arc::make_mut`.
    pub fn lease(&self) -> ParticipantLease {
        ParticipantLease {
            id: self.id,
            history: self.history.clone(),
        }
    }
}

/// The activity history of a participant, stored behind `Arc` for Copy-on-Write sharing.
///
/// Updates use `Arc::make_mut` so that the vector is mutated in place when no
/// `Arc` reference is held (refcount == 1), and cloned only when a reference
/// exists (e.g. a slow reporter's buffered `Updated` message or a held lease).
///
/// No-op updates (state unchanged) return before calling `make_mut`, avoiding
/// clones even when the refcount is > 1.  See [`Room::lease`] for the full
/// rationale and the `PreUpdate` optimization.
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

#[cfg(test)]
mod cow_tests {
    use super::*;
    use serenity::all::{ChannelId, GuildId, UserId};
    use tokio::time::Instant;

    fn make_room() -> Room {
        use crate::room::RoomIdGenerator;
        let mut id_gen = RoomIdGenerator::new();
        Room::new(id_gen.next_id(), GuildId::new(1), ChannelId::new(2), Moment::now())
    }

    fn make_participant() -> Participant {
        let mut p = Participant::new(UserId::new(1));
        p.connect(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();
        p
    }

    #[test]
    fn update_audio_same_state_does_not_clone() {
        let mut p = make_participant();
        // Hold a lease (refcount = 2)
        let lease = p.lease();
        let ptr_before = Arc::as_ptr(&p.history.audio);

        // Update with same state (no-op)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.audio);
        assert_eq!(ptr_before, ptr_after, "no-op update should not clone");
        drop(lease);
    }

    #[test]
    fn update_audio_different_state_clones() {
        let mut p = make_participant();
        let lease = p.lease();
        let ptr_before = Arc::as_ptr(&p.history.audio);

        // Update with different state (muted changed)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: true,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.audio);
        assert_ne!(ptr_before, ptr_after, "state change should clone when lease held");
        drop(lease);
    }

    #[test]
    fn update_audio_same_state_no_lease_does_not_clone() {
        let mut p = make_participant();
        let ptr_before = Arc::as_ptr(&p.history.audio);

        // Update with same state, no lease held (no-op)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.audio);
        assert_eq!(ptr_before, ptr_after, "no-op without lease should not clone (in-place)");
    }

    #[test]
    fn update_audio_state_change_no_lease_does_not_clone() {
        let mut p = make_participant();
        let ptr_before = Arc::as_ptr(&p.history.audio);

        // Update with different state, no lease held (state change)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: true,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.audio);
        assert_eq!(ptr_before, ptr_after, "state change without lease should be in-place (no clone)");
    }

    #[test]
    fn connect_returns_invalid_state_when_already_connected() {
        let mut p = make_participant();
        let result = p.connect(Instant::now(), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        assert!(matches!(result, Err(ParticipantError::InvalidState)),
            "connecting an already-connected participant should fail");
    }

    #[test]
    fn update_returns_invalid_state_when_not_connected() {
        let p_id = UserId::new(1);
        let mut p = Participant::new(p_id);
        // Not connected yet
        let result = p.update(Instant::now(), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        assert!(matches!(result, Err(ParticipantError::InvalidState)),
            "updating a non-connected participant should fail");
    }

    #[test]
    fn disconnect_returns_invalid_state_when_not_connected() {
        let p_id = UserId::new(1);
        let mut p = Participant::new(p_id);
        let result = p.disconnect(Instant::now());
        assert!(matches!(result, Err(ParticipantError::InvalidState)),
            "disconnecting a non-connected participant should fail");
    }

    #[test]
    fn screen_sharing_start_stop_splits_interval() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: true,
        });
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        assert_eq!(p.history.screen_sharing.len(), 1);
        assert!(p.history.screen_sharing[0].is_ended(),
            "screen sharing interval should be ended after stop");
    }

    #[test]
    fn screen_sharing_stop_without_prior_is_noop() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        assert_eq!(p.history.screen_sharing.len(), 0,
            "no screen sharing activity should exist");
    }

    #[test]
    fn screen_sharing_restart_creates_new_interval() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: true,
        });
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        room.handle_voice_state(now, UserId::new(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: true,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        assert_eq!(p.history.screen_sharing.len(), 2,
            "restart should create a new interval");
    }

    #[test]
    fn held_lease_unaffected_by_participant_update() {
        let mut p = make_participant();
        let lease = p.lease();
        let lease_audio_len_before = lease.history.audio.len();

        // Update with state change while lease is held
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: true,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        // Lease should still hold the old data (CoW isolation)
        assert_eq!(lease.history.audio.len(), lease_audio_len_before,
            "held lease must not see the participant's mutation");
        assert!(!lease.history.audio.last().unwrap().muted,
            "held lease must retain old mute state");

        // Participant should have the new state
        assert!(p.history.audio.last().unwrap().muted,
            "participant should have new mute state");
        assert!(p.history.audio.len() > lease.history.audio.len(),
            "participant should have more activities after update");

        drop(lease);
    }

    #[test]
    fn update_screen_sharing_noop_does_not_clone() {
        let mut p = make_participant();
        let lease = p.lease();
        let ptr_before = Arc::as_ptr(&p.history.screen_sharing);

        // No sharing → no sharing (no-op)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.screen_sharing);
        assert_eq!(ptr_before, ptr_after, "no-op screen sharing should not clone");
        drop(lease);
    }

    #[test]
    fn update_screen_sharing_change_clones() {
        // First, start sharing (no lease held)
        let mut p = make_participant();
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: true,
        }).unwrap();
        // Now hold a lease
        let lease = p.lease();
        let ptr_before = Arc::as_ptr(&p.history.screen_sharing);

        // Stop sharing (state change → should clone)
        p.update(Instant::now(), VoiceStateFlags {
            is_muted: false,
            is_deafened: false,
            is_sharing_screen: false,
        }).unwrap();

        let ptr_after = Arc::as_ptr(&p.history.screen_sharing);
        assert_ne!(ptr_before, ptr_after, "screen sharing change should clone when lease held");
        // Lease should still hold old state (1 ongoing interval)
        assert_eq!(lease.history.screen_sharing.len(), 1);
        assert!(lease.history.screen_sharing[0].is_ongoing(),
            "held lease must retain ongoing interval");
        drop(lease);
    }
}

#[cfg(test)]
mod room_tests {
    use super::*;
    use serenity::all::UserId;
    use tokio::time::Instant;

    fn make_room() -> Room {
        use crate::room::RoomIdGenerator;
        let mut id_gen = RoomIdGenerator::new();
        Room::new(id_gen.next_id(), serenity::all::GuildId::new(1), serenity::all::ChannelId::new(2), Moment::now())
    }

    fn uid(n: u64) -> UserId {
        UserId::new(n)
    }

    #[test]
    fn connect_creates_participant() {
        let mut room = make_room();
        room.handle_voice_state(Instant::now(), uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        assert!(room.is_connected(uid(1)));
        assert_eq!(room.participants.len(), 1);
    }

    #[test]
    fn connect_twice_for_same_user_reconnects() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        // disconnect
        let status = room.handle_disconnect(now, uid(1)).unwrap();
        assert_eq!(status, RoomStatus::Empty);
        assert!(!room.is_connected(uid(1)));
        // reconnect
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: true, is_deafened: false, is_sharing_screen: false,
        });
        assert!(room.is_connected(uid(1)));
        assert_eq!(room.participants.len(), 1); // same participant, not duplicated
        let lease = room.lease();
        assert_eq!(lease.participants[0].history.audio.len(), 2,
            "reconnect should create a new audio activity");
    }

    #[test]
    fn disconnect_returns_empty_when_last_leaves() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let status = room.handle_disconnect(now, uid(1)).unwrap();
        assert_eq!(status, RoomStatus::Empty);
        assert!(room.is_empty(), "room should report empty after last participant leaves");
    }

    #[test]
    fn disconnect_returns_occupied_when_others_remain() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        room.handle_voice_state(now, uid(2), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let status = room.handle_disconnect(now, uid(1)).unwrap();
        assert_eq!(status, RoomStatus::Occupied);
    }

    #[test]
    fn voice_state_update_splits_audio_activity_on_mute_change() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: true, is_deafened: false, is_sharing_screen: false,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        // Should have 2 audio activities (initial + mute change)
        assert_eq!(p.history.audio.len(), 2);
        assert!(p.history.audio[1].muted);
    }

    #[test]
    fn voice_state_update_same_state_does_not_split_audio_activity() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        // Same state → no split, still 1 activity
        assert_eq!(p.history.audio.len(), 1);
    }

    #[test]
    fn handle_disconnect_unknown_participant_returns_error() {
        let mut room = make_room();
        let now = Instant::now();
        // Disconnect for unknown user returns ParticipantNotFound (Err-based design).
        // Session handles this as a no-op (warn + continue).
        let result = room.handle_disconnect(now, uid(99));
        assert!(matches!(result, Err(RoomError::ParticipantNotFound)));
    }

    #[test]
    fn room_is_empty_on_creation() {
        let room = make_room();
        assert!(room.is_empty());
    }

    #[test]
    fn is_connected_returns_false_for_unknown_user() {
        let room = make_room();
        assert!(!room.is_connected(uid(1)));
    }

    #[test]
    fn lease_captures_state() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: false,
        });
        let lease = room.lease();
        assert_eq!(lease.participants.len(), 1);
        assert_eq!(lease.channel_id, room.channel_id);
    }

    #[test]
    fn screen_sharing_start_creates_ongoing_interval() {
        let mut room = make_room();
        let now = Instant::now();
        room.handle_voice_state(now, uid(1), VoiceStateFlags {
            is_muted: false, is_deafened: false, is_sharing_screen: true,
        });
        let lease = room.lease();
        let p = &lease.participants[0];
        assert_eq!(p.history.screen_sharing.len(), 1);
        assert!(p.history.screen_sharing[0].is_ongoing());
    }
}
