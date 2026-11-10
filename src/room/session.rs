use crate::room::coordinator::CoordinatorInternalMessage;
use crate::room::model::{Room, RoomStatus};
use crate::room::types::VoiceStateFlags;
use crate::room::{Moment, RoomLease};
use better_tokio_select::tokio_select;
use serenity::all::UserId;

use std::time::Duration;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tracing::{error, info, warn};

/// A smart proxy for communicating with a [`Session`].
///
/// Beyond simple message passing, `SessionHandle` can "suspend" delivery,
/// buffering incoming messages in memory. This is crucial during the shutdown
/// sequence to prevent race conditions.
pub struct SessionHandle {
    suspended_events: Option<Vec<SessionMessage>>,
    tx: mpsc::UnboundedSender<SessionMessage>,
}

impl SessionHandle {
    pub fn new(tx: mpsc::UnboundedSender<SessionMessage>) -> Self {
        Self {
            tx,
            suspended_events: None,
        }
    }

    /// Dispatches a message to the session.
    ///
    /// If message delivery is currently suspended, the message will be queued in memory.
    /// Otherwise, it is sent immediately to the session task.
    pub fn dispatch(&mut self, event: SessionMessage) -> Result<(), SendError<SessionMessage>> {
        match self.suspended_events.as_mut() {
            Some(queue) => queue.push(event),
            None => self.tx.send(event)?,
        }
        Ok(())
    }

    /// Forces immediate delivery of a message, bypassing any suspension buffers.
    ///
    /// This should be used for critical control signals (like shutdown requests)
    /// that must be processed even when the session's normal event flow is suspended.
    pub fn force_dispatch(&self, event: SessionMessage) -> Result<(), SendError<SessionMessage>> {
        self.tx.send(event)?;
        Ok(())
    }

    /// Suspends message delivery and starts buffering all subsequent messages.
    pub fn suspend_delivery(&mut self) {
        if self.suspended_events.is_none() {
            self.suspended_events = Some(Vec::new());
        }
    }

    /// Resumes message delivery and flushes all buffered messages to the session.
    pub fn resume_delivery(&mut self) -> Result<(), SendError<SessionMessage>> {
        if let Some(queue) = self.suspended_events.take() {
            for event in queue {
                self.tx.send(event)?;
            }
        }
        Ok(())
    }

    /// Reconnects the handle to a new session instance.
    ///
    /// Used when a session has shut down but new events were buffered, requiring
    /// a fresh session to process them.
    pub fn reconnect(&mut self, new_tx: mpsc::UnboundedSender<SessionMessage>) {
        self.tx = new_tx;
    }

    /// Returns true if message delivery is currently suspended.
    pub fn is_suspended(&self) -> bool {
        self.suspended_events.is_some()
    }

    /// Returns true if there are any events currently waiting in the buffer.
    ///
    /// This is typically checked after a session shutdown to see if any new events
    /// arrived during the shutdown process, requiring a fresh session to be spawned.
    pub fn has_waiting_events(&self) -> bool {
        self.suspended_events
            .as_ref()
            .is_some_and(|events| !events.is_empty())
    }

    /// Returns the minimum `now` among buffered `VoiceStateUpdate` messages.
    ///
    /// Used when reconnecting to seed the new Room's start instant so replayed
    /// events (whose Instants originate from the previous session) land within
    /// the new Room's timeline instead of after a fresh `Moment::now()`.
    pub fn earliest_pending_instant(&self) -> Option<Instant> {
        self.suspended_events.as_ref().and_then(|events| {
            events
                .iter()
                .filter_map(|event| match event {
                    SessionMessage::VoiceStateUpdate(vsu) => Some(vsu.now),
                    _ => None,
                })
                .min()
        })
    }
}

/// Reasons why a session might be requested to shut down.
#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownReason {
    /// The channel has been empty for a certain duration.
    Idle,
}

/// Messages sent from the coordinator to a session.
pub enum SessionMessage {
    VoiceStateUpdate(VoiceStateUpdate),
    /// Request the session to shut down gracefully.
    RequestShutdown {
        reason: ShutdownReason,
        end: Moment,
    },
}

pub struct VoiceStateUpdate {
    pub now: Instant,
    pub user_id: UserId,
    pub flags: Option<VoiceStateFlags>,
}

/// Events emitted by a session to its subscribers (e.g., for reporting).
#[derive(Clone)]
pub enum SessionEvent {
    /// Sent before the state mutation to evict the previous `Updated` from
    /// the broadcast buffer (capacity 1), dropping its `Arc` reference so
    /// [`Room::lease`]'s Copy-on-Write mutation stays O(1). See
    /// [`Room::lease`] for the full contract.
    ///
    /// Lag detection: `PreUpdate` and `Updated` are sent as a pair; on
    /// `Lagged(n)`, `(n - 1) / 2` estimates skipped update pairs.
    PreUpdate,
    /// The session's internal state (room model) has been updated.
    ///
    /// Carries a [`RoomLease`] whose history vectors are behind `Arc`. The
    /// Reporter should convert to owned data (`RoomSnapshot`) promptly so the
    /// `Arc` reference is released and the Room task can mutate in place.
    Updated {
        room: RoomLease,
        /// Monotonic application-level version, incremented only on changed==true,
        /// used by Reporter for render gating.
        mutation_id: u64,
    },
    /// The session has finished and is now closed.
    Shutdown { room: RoomLease, end: Moment },
}

/// Sender side of session events.
///
/// Abstracts the broadcast channel so the driver can evict the previous
/// `Updated` (Clear) before mutating, share a new lease (Share), or
/// finalize (Finish) — without knowing the channel's capacity or the
/// `PreUpdate` implementation detail. Mockable for tests.
pub trait SessionEventSink {
    /// Evict the previous `Updated` from the broadcast buffer (CoW).
    fn evict(&self);
    /// Publish the new room state to subscribers.
    fn publish(&self, room: RoomLease, mutation_id: u64);
    /// Finalize the session (shutdown).
    fn shutdown(&self, room: RoomLease, end: Moment);
}

impl SessionEventSink for broadcast::Sender<SessionEvent> {
    fn evict(&self) {
        let _ = self.send(SessionEvent::PreUpdate);
    }
    fn publish(&self, room: RoomLease, mutation_id: u64) {
        let _ = self.send(SessionEvent::Updated { room, mutation_id });
    }
    fn shutdown(&self, room: RoomLease, end: Moment) {
        let _ = self.send(SessionEvent::Shutdown { room, end });
    }
}

/// Pure state for a single voice channel session.
///
/// Holds the Room and idle tracking state. The state machine mutates state
/// and returns domain outcomes; it performs no I/O. The driver (Session::run)
/// owns all I/O and maps outcomes to SessionEvent / CoordinatorInternalMessage.
pub struct SessionState {
    room: Room,
    idle: IdleTracker,
}

impl SessionState {
    pub fn new(room: Room, idle_timeout: Duration) -> Self {
        Self {
            room,
            idle: IdleTracker::new(idle_timeout),
        }
    }

    pub fn idle_deadline(&self) -> Option<Instant> {
        self.idle.deadline()
    }

    pub fn idle_has_fired(&self) -> bool {
        self.idle.has_fired()
    }

    /// Domain outcome of a voice state update.
    pub fn on_voice_state_update(&mut self, vsu: VoiceStateUpdate) -> VoiceStateOutcome {
        if let Some(flags) = vsu.flags {
            let changed = self.room.handle_voice_state(vsu.now, vsu.user_id, flags);
            self.idle.abort(); // abort is a no-op when no countdown is running
            VoiceStateOutcome::Updated { changed }
        } else {
            match self.room.handle_disconnect(vsu.now, vsu.user_id) {
                Ok(status) => {
                    if status == RoomStatus::Empty {
                        self.idle.start_countdown(vsu.now);
                    }
                    VoiceStateOutcome::Updated { changed: true }
                }
                Err(_) => VoiceStateOutcome::UnknownParticipant,
            }
        }
    }

    /// Domain outcome of a shutdown request.
    pub fn on_shutdown_requested(
        &mut self,
        reason: ShutdownReason,
    ) -> ShutdownOutcome {
        if !self.room.is_empty() {
            return ShutdownOutcome::Rejected;
        }
        if reason == ShutdownReason::Idle && !self.idle.has_fired() {
            return ShutdownOutcome::Rejected;
        }
        ShutdownOutcome::Accepted
    }

    /// Fires the idle timeout and returns the start instant for the Idle message.
    pub fn on_idle_timeout(&mut self, now: Instant) -> Instant {
        self.idle.fire(now)
    }
}

/// Domain outcome of a voice state update.
#[derive(Debug, PartialEq, Eq)]
pub enum VoiceStateOutcome {
    /// State changed; subscribers should be notified.
    Updated {
        /// Whether state changed. Drives the monotonic application-level
        /// mutation_id (incremented only on changed==true), used by Reporter
        /// for render gating.
        changed: bool,
    },
    /// Disconnect for an unknown participant; no notification needed.
    UnknownParticipant,
}

/// Domain outcome of a shutdown request.
#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Rejected,
    Accepted,
}

/// An actor-like task that manages the state of a single voice channel activity.
pub struct Session<S: SessionEventSink> {
    state: SessionState,
    rx: mpsc::UnboundedReceiver<SessionMessage>,
    coordinator_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
    event_sink: S,
    /// Monotonic application-level version, incremented only on changed==true,
    /// used by Reporter for render gating.
    mutation_id: u64,
}

impl<S: SessionEventSink> Session<S> {
    pub fn new(
        room: Room,
        rx: mpsc::UnboundedReceiver<SessionMessage>,
        coordinator_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
        event_sink: S,
        idle_timeout: Duration,
    ) -> Session<S> {
        Session {
            state: SessionState::new(room, idle_timeout),
            rx,
            coordinator_tx,
            event_sink,
            mutation_id: 0,
        }
    }

    #[tracing::instrument(
        name = "session",
        skip(self),
        fields(
            channel = %self.state.room.channel_id,
            id = %self.state.room.id,
        )
    )]
    pub async fn run(mut self) {
        info!("Session started");

        loop {
            let idle_deadline = self.state.idle_deadline();
            tokio_select!(
                biased,
                match .. {
                    .. if let result = self.rx.recv() => {
                        let cmd = match result {
                            Some(cmd) => cmd,
                            None => {
                                info!("Session command channel closed, stopping");
                                break;
                            }
                        };
                        match cmd {
                            SessionMessage::VoiceStateUpdate(vsu) => {
                                self.event_sink.evict();
                                match self.state.on_voice_state_update(vsu) {
                                    VoiceStateOutcome::Updated { changed } => {
                                        if changed {
                                            self.mutation_id += 1;
                                        }
                                        self.event_sink.publish(self.state.room.lease(), self.mutation_id);
                                    }
                                    VoiceStateOutcome::UnknownParticipant => {
                                        warn!("room inconsistency: unknown participant on disconnect");
                                        self.event_sink.publish(self.state.room.lease(), self.mutation_id);
                                    }
                                }
                            }
                            SessionMessage::RequestShutdown { reason, end } => {
                                let channel_id = self.state.room.channel_id;
                                let guild_id = self.state.room.guild_id;
                                match self.state.on_shutdown_requested(reason) {
                                    ShutdownOutcome::Rejected => {
                                        if let Err(err) = self.coordinator_tx.send(
                                            CoordinatorInternalMessage::RejectShutdown { channel_id },
                                        ) {
                                            error!(error = %err, "Failed to send shutdown rejection");
                                            self.event_sink.shutdown(self.state.room.lease(), end);
                                            info!("Session stopped");
                                            break;
                                        }
                                    }
                                    ShutdownOutcome::Accepted => {
                                        self.event_sink.shutdown(self.state.room.lease(), end);
                                        if let Err(err) = self.coordinator_tx.send(
                                            CoordinatorInternalMessage::AcceptShutdown {
                                                channel_id,
                                                guild_id,
                                            },
                                        ) {
                                            error!(error = %err, "Failed to send shutdown acceptance");
                                        }
                                        info!("Session stopped");
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    .. if let _ = async {
                        match idle_deadline {
                            Some(d) => tokio::time::sleep_until(d).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if idle_deadline.is_some() {
                            info!("Detected idle, starting idle-shutdown sequence...");
                            let start = self.state.on_idle_timeout(tokio::time::Instant::now());
                            let channel_id = self.state.room.channel_id;
                            let since = self.state.room.start.at(start);
                            if let Err(err) = self.coordinator_tx.send(
                                CoordinatorInternalMessage::Idle { channel_id, since },
                            ) {
                                error!(error = %err, "Failed to send idle notification");
                                info!("Session stopped");
                                break;
                            }
                        }
                    }
                }
            )
        }
    }
}

struct IdleTracker {
    start: Option<Instant>,
    deadline: Option<Instant>,
    notified: bool,
    timeout: Duration,
}

impl IdleTracker {
    fn new(timeout: Duration) -> Self {
        Self {
            start: None,
            deadline: None,
            notified: false,
            timeout,
        }
    }

    fn start_countdown(&mut self, now: Instant) {
        self.start = Some(now);
        self.deadline = Some(now + self.timeout);
        self.notified = false;
    }

    fn abort(&mut self) {
        self.start = None;
        self.deadline = None;
        self.notified = false;
    }

    fn has_fired(&self) -> bool {
        self.notified
    }

    fn deadline(&self) -> Option<Instant> {
        if self.notified {
            None
        } else {
            self.deadline
        }
    }

    /// Marks the countdown as fired: records the start time and clears the deadline.
    fn fire(&mut self, now: Instant) -> Instant {
        let start = self.start.take().unwrap_or(now);
        self.notified = true;
        self.deadline = None;
        start
    }
}

#[cfg(test)]
mod session_smoke_tests {
    use super::*;
    use serenity::all::{ChannelId, GuildId, UserId};
    use tokio::sync::mpsc;
    use tokio::time::Instant;

    #[tokio::test]
    async fn session_emits_updated_on_connect() {
        use crate::room::RoomIdGenerator;
        let mut id_gen = RoomIdGenerator::new();
        let room = Room::new(
            id_gen.next_id(),
            GuildId::new(1),
            ChannelId::new(2),
            Moment::now(),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let (coord_tx, _coord_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(4);

        let session = Session::new(room, rx, coord_tx, event_tx, Duration::from_secs(60));
        tokio::spawn(session.run());

        tx.send(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags {
                is_muted: false,
                is_deafened: false,
                is_sharing_screen: false,
            }),
        })).unwrap();

        // Skip PreUpdate
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            event_rx.recv(),
        ).await.unwrap().unwrap();

        let event = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            event_rx.recv(),
        ).await.unwrap().unwrap();

        match event {
            SessionEvent::Updated { room, .. } => {
                assert_eq!(room.participants.len(), 1);
            }
            other => panic!("expected Updated, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[tokio::test]
    async fn session_emits_updated_on_disconnect() {
        use crate::room::RoomIdGenerator;
        let mut id_gen = RoomIdGenerator::new();
        let room = Room::new(
            id_gen.next_id(),
            GuildId::new(1),
            ChannelId::new(2),
            Moment::now(),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let (coord_tx, _coord_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(4);

        let session = Session::new(room, rx, coord_tx, event_tx, Duration::from_secs(60));
        tokio::spawn(session.run());

        // Connect first
        tx.send(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags {
                is_muted: false,
                is_deafened: false,
                is_sharing_screen: false,
            }),
        })).unwrap();

        // Consume PreUpdate + Updated from connect
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv()).await.unwrap().unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv()).await.unwrap().unwrap();

        // Disconnect
        tx.send(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();

        // Skip PreUpdate
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv()).await.unwrap().unwrap();

        let event = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            event_rx.recv(),
        ).await.unwrap().unwrap();

        match event {
            SessionEvent::Updated { room, .. } => {
                assert_eq!(room.participants.len(), 1); // participant still tracked
            }
            other => panic!("expected Updated, got {:?}", std::mem::discriminant(&other)),
        }
    }
}

#[cfg(test)]
mod session_handle_tests {
    use super::*;
    use serenity::all::UserId;
    use tokio::sync::mpsc;

    fn make_handle() -> (SessionHandle, mpsc::UnboundedReceiver<SessionMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (SessionHandle::new(tx), rx)
    }

    #[test]
    fn dispatch_sends_when_not_suspended() {
        let (mut handle, mut rx) = make_handle();
        handle.dispatch(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: tokio::time::Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();
        assert!(rx.try_recv().is_ok(), "message should be delivered immediately");
    }

    #[test]
    fn dispatch_buffers_when_suspended() {
        let (mut handle, mut rx) = make_handle();
        handle.suspend_delivery();
        handle.dispatch(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: tokio::time::Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();
        assert!(rx.try_recv().is_err(), "message should be buffered, not delivered");
        assert!(handle.has_waiting_events());
    }

    #[test]
    fn resume_flushes_buffered_messages() {
        let (mut handle, mut rx) = make_handle();
        handle.suspend_delivery();
        handle.dispatch(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: tokio::time::Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();
        handle.resume_delivery().unwrap();
        assert!(rx.try_recv().is_ok(), "buffered message should be delivered");
        assert!(!handle.has_waiting_events());
    }

    #[test]
    fn force_dispatch_bypasses_suspension() {
        let (mut handle, mut rx) = make_handle();
        handle.suspend_delivery();
        handle.force_dispatch(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: tokio::time::Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();
        assert!(rx.try_recv().is_ok(), "force_dispatch should bypass buffer");
    }

    #[test]
    fn reconnect_retakes_tx() {
        let (mut handle, _rx) = make_handle();
        let (new_tx, mut new_rx) = mpsc::unbounded_channel();
        handle.reconnect(new_tx);
        handle.dispatch(SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
            now: tokio::time::Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        })).unwrap();
        assert!(new_rx.try_recv().is_ok(), "after reconnect, messages go to new tx");
    }

    #[test]
    fn is_suspended_reflects_state() {
        let (mut handle, _rx) = make_handle();
        assert!(!handle.is_suspended());
        handle.suspend_delivery();
        assert!(handle.is_suspended());
        handle.resume_delivery().unwrap();
        assert!(!handle.is_suspended());
    }
}

#[cfg(test)]
mod session_state_tests {
    use super::*;
    use serenity::all::{ChannelId, GuildId, UserId};
    use tokio::time::Instant;

    fn make_room() -> Room {
        use crate::room::RoomIdGenerator;
        let mut id_gen = RoomIdGenerator::new();
        Room::new(id_gen.next_id(), GuildId::new(1), ChannelId::new(2), Moment::now())
    }

    fn make_state() -> SessionState {
        SessionState::new(make_room(), Duration::from_secs(60))
    }

    #[test]
    fn connect_emits_updated() {
        let mut state = make_state();
        let outcome = state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags {
                is_muted: false,
                is_deafened: false,
                is_sharing_screen: false,
            }),
        });
        assert_eq!(outcome, VoiceStateOutcome::Updated { changed: true });
        assert_eq!(state.room.participants.len(), 1);
    }

    #[test]
    fn connect_aborts_idle() {
        let mut state = make_state();
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags { is_muted: false, is_deafened: false, is_sharing_screen: false }),
        });
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        });
        assert!(state.idle_deadline().is_some());
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags { is_muted: false, is_deafened: false, is_sharing_screen: false }),
        });
        assert!(state.idle_deadline().is_none());
        assert!(!state.idle_has_fired());
    }

    #[test]
    fn disconnect_empty_starts_idle() {
        let mut state = make_state();
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags { is_muted: false, is_deafened: false, is_sharing_screen: false }),
        });
        let outcome = state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        });
        assert_eq!(outcome, VoiceStateOutcome::Updated { changed: true });
        assert!(state.idle_deadline().is_some());
    }

    #[test]
    fn disconnect_unknown_returns_unknown_participant() {
        let mut state = make_state();
        let outcome = state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(99),
            flags: None,
        });
        assert_eq!(outcome, VoiceStateOutcome::UnknownParticipant);
    }

    #[test]
    fn request_shutdown_non_empty_rejects() {
        let mut state = make_state();
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags { is_muted: false, is_deafened: false, is_sharing_screen: false }),
        });
        let outcome = state.on_shutdown_requested(ShutdownReason::Idle);
        assert_eq!(outcome, ShutdownOutcome::Rejected);
    }

    #[test]
    fn request_shutdown_empty_not_fired_rejects() {
        let mut state = make_state();
        let outcome = state.on_shutdown_requested(ShutdownReason::Idle);
        assert_eq!(outcome, ShutdownOutcome::Rejected);
    }

    #[test]
    fn request_shutdown_empty_fired_accepts() {
        let mut state = make_state();
        state.on_idle_timeout(Instant::now());
        let outcome = state.on_shutdown_requested(ShutdownReason::Idle);
        assert_eq!(outcome, ShutdownOutcome::Accepted);
    }

    #[test]
    fn idle_timeout_returns_start_instant() {
        let mut state = make_state();
        // connect then disconnect to reach empty
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: Some(VoiceStateFlags { is_muted: false, is_deafened: false, is_sharing_screen: false }),
        });
        state.on_voice_state_update(VoiceStateUpdate {
            now: Instant::now(),
            user_id: UserId::new(1),
            flags: None,
        });
        let start = state.on_idle_timeout(Instant::now());
        // state.room.start.at(start) should be a valid Moment
        let _ = state.room.start.at(start);
    }
}

#[cfg(test)]
mod idle_tracker_tests {
    use super::*;
    use tokio::time::Instant;

    #[test]
    fn start_countdown_sets_deadline() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));
        let now = Instant::now();
        tracker.start_countdown(now);
        assert!(tracker.deadline().is_some());
        assert!(!tracker.has_fired());
    }

    #[test]
    fn abort_clears_state() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));
        tracker.start_countdown(Instant::now());
        tracker.abort();
        assert!(tracker.deadline().is_none());
        assert!(!tracker.has_fired());
    }

    #[test]
    fn fire_sets_notified() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));
        let now = Instant::now();
        tracker.start_countdown(now);
        let start = tracker.fire(now);
        assert!(tracker.has_fired());
        assert!(tracker.deadline().is_none());
        assert_eq!(start, now);
    }

    #[test]
    fn fire_uses_fallback_when_start_is_none() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));
        let now = Instant::now();
        let start = tracker.fire(now);
        assert_eq!(start, now);
        assert!(tracker.has_fired());
    }

    #[test]
    fn deadline_returns_none_after_fire() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));
        tracker.start_countdown(Instant::now());
        tracker.fire(Instant::now());
        assert!(tracker.deadline().is_none());
    }
}