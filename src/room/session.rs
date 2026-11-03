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
}

/// Reasons why a session might be requested to shut down.
#[derive(Debug, PartialEq)]
pub enum ShutdownReason {
    /// The channel has been empty for a certain duration.
    Idle,
    /// An external command or system event requested shutdown.
    External,
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
    /// The session is about to update its internal state.
    ///
    /// # Copy-on-Write optimization
    ///
    /// `Room` stores activity history behind `Arc<Vec<...>>`.  When a reporter
    /// is slow, the previous [`Updated`](SessionEvent::Updated) message (which
    /// carries a `RoomLease` with an `Arc` clone) may still sit in the
    /// broadcast channel's internal buffer.  While that `Arc` is alive, the
    /// Room task's `Arc::make_mut` must clone the whole vector on every
    /// update — O(n) instead of O(1).
    ///
    /// `PreUpdate` is sent **before** the state mutation.  Because the
    /// broadcast channel has capacity 1, sending `PreUpdate` (which carries no
    /// `Arc`) pushes the previous `Updated` out of the buffer, dropping its
    /// `Arc` reference.  By the time the Room task mutates, the `Arc`'s
    /// strong count is 1 and `make_mut` works in place.
    ///
    /// The Reporter does not need to handle `PreUpdate` explicitly — the
    /// buffer eviction happens automatically when the message is sent.
    ///
    /// # Secondary use: Lag detection
    ///
    /// `PreUpdate` and `Updated` are sent as a pair.  When the Reporter
    /// lags, the broadcast channel reports `Lagged(n)`; dividing by 2
    /// (`(n - 1) / 2`) estimates how many update pairs were skipped.
    PreUpdate,
    /// The session's internal state (room model) has been updated.
    ///
    /// Carries a [`RoomLease`] whose history vectors are behind `Arc`.  The
    /// Reporter should convert to owned data (`RoomSnapshot`) promptly so the
    /// `Arc` reference is released and the Room task can mutate in place.
    Updated { room: RoomLease },
    /// The session has finished and is now closed.
    Shutdown { room: RoomLease, end: Moment },
}

/// An actor-like task that manages the state of a single voice channel activity.
///
/// `Session` tracks who is in the room, their voice states, and records the
/// history of these activities. It also manages its own idle timeout.
pub struct Session {
    room: Room,
    rx: mpsc::UnboundedReceiver<SessionMessage>,
    coordinator_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
    event_tx: broadcast::Sender<SessionEvent>,
}

impl Session {
    pub fn new(
        room: Room,
        rx: mpsc::UnboundedReceiver<SessionMessage>,
        coordinator_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
        event_tx: broadcast::Sender<SessionEvent>,
    ) -> Session {
        Session {
            room,
            rx,
            coordinator_tx,
            event_tx,
        }
    }

    #[tracing::instrument(
        name = "session",
        skip(self),
        fields(
            channel = %self.room.channel_id,
            id = %self.room.id,
        )
    )]
    pub async fn run(mut self) {
        info!("Session started");

        let mut idle = IdleTracker::new();

        loop {
            tokio_select!(
                biased,
                match .. {
                    .. if let Some(cmd) = self.rx.recv() => {
                        match cmd {
                            SessionMessage::VoiceStateUpdate(voice_state_update) => {
                                let Some(flags) = voice_state_update.flags else {
                                    info!(user_id = %voice_state_update.user_id, "Participant disconnected");
                                    let _ = self.event_tx.send(SessionEvent::PreUpdate);
                                    let status = match self.room.handle_disconnect(
                                        voice_state_update.now,
                                        voice_state_update.user_id,
                                    ) {
                                        Ok(status) => status,
                                        Err(error) => {
                                            warn!(
                                                ?error,
                                                "room inconsistency was detected. just ignored the update."
                                            );
                                            continue;
                                        }
                                    };
                                    if status == RoomStatus::Empty {
                                        info!("Room is now empty, starting idle countdown");
                                        idle.start_countdown(voice_state_update.now);
                                    }
                                    let _ = self.event_tx.send(SessionEvent::Updated {
                                        room: self.room.lease(),
                                    });

                                    continue;
                                };

                                let was_connected = self.room.is_connected(voice_state_update.user_id);
                                if was_connected {
                                    info!(user_id = %voice_state_update.user_id, flags = ?voice_state_update.flags, "Participant state updated");
                                } else {
                                    info!(user_id = %voice_state_update.user_id, "Participant connected");
                                }
                                let _ = self.event_tx.send(SessionEvent::PreUpdate);
                                self.room.handle_voice_state(
                                    voice_state_update.now,
                                    voice_state_update.user_id,
                                    flags,
                                );
                                if !was_connected {
                                    idle.abort();
                                }
                                let _ = self.event_tx.send(SessionEvent::Updated {
                                    room: self.room.lease(),
                                });
                            }
                            SessionMessage::RequestShutdown { reason, end } => {
                                if !self.room.is_empty() {
                                    warn!(
                                        ?reason,
                                        "Shutdown requested but room is not empty. Rejecting."
                                    );
                                    if let Err(err) = self.coordinator_tx.send(
                                        CoordinatorInternalMessage::RejectShutdown {
                                            channel_id: self.room.channel_id,
                                        },
                                    ) {
                                        error!(error = %err, "Failed to send shutdown rejection");
                                        let _ = self.event_tx.send(SessionEvent::Shutdown {
                                            room: self.room.lease(),
                                            end,
                                        });
                                        break;
                                    }
                                    continue;
                                }

                                if reason == ShutdownReason::Idle
                                    && !idle.has_expired()
                                {
                                    info!(
                                        "Shutdown requested but room is recently active. Rejecting."
                                    );
                                    if let Err(err) = self.coordinator_tx.send(
                                        CoordinatorInternalMessage::RejectShutdown {
                                            channel_id: self.room.channel_id,
                                        },
                                    ) {
                                        error!(error = %err, "Failed to send shutdown rejection");
                                        let _ = self.event_tx.send(SessionEvent::Shutdown {
                                            room: self.room.lease(),
                                            end,
                                        });
                                        break;
                                    }
                                    continue;
                                }

                                let _ = self.event_tx.send(SessionEvent::Shutdown {
                                    room: self.room.lease(),
                                    end,
                                });
                                if let Err(err) = self.coordinator_tx.send(
                                    CoordinatorInternalMessage::AcceptShutdown {
                                        channel_id: self.room.channel_id,
                                        room: self.room,
                                    },
                                ) {
                                    error!(error = %err, "Failed to send shutdown acceptance");
                                }

                                info!("Session stopped");
                                break;
                            }
                        }
                    }

                    .. if let _ = async {
                        match idle.deadline() {
                            Some(d) => tokio::time::sleep_until(d).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if idle.deadline().is_some() {
                            let start = idle.fire(Instant::now());
                            info!(idle_since = ?start, "Detected idle, starting idle-shutdown sequence...");
                            if let Err(err) = self.coordinator_tx.send(
                                CoordinatorInternalMessage::Idle {
                                    channel_id: self.room.channel_id,
                                    since: self.room.start.at(start),
                                },
                            ) {
                                error!(error = %err, "Failed to send idle notification");
                                break;
                            }
                        }
                    }
                }
            )
        }
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

        let session = Session::new(room, rx, coord_tx, event_tx);
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
            SessionEvent::Updated { room } => {
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

        let session = Session::new(room, rx, coord_tx, event_tx);
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
            SessionEvent::Updated { room } => {
                assert_eq!(room.participants.len(), 1); // participant still tracked
            }
            other => panic!("expected Updated, got {:?}", std::mem::discriminant(&other)),
        }
    }
}

struct IdleTracker {
    start: Option<Instant>,
    deadline: Option<Instant>,
    notified: bool,
}

impl IdleTracker {
    const TIMEOUT: Duration = Duration::from_secs(60);

    fn new() -> Self {
        Self {
            start: None,
            deadline: None,
            notified: false,
        }
    }

    fn start_countdown(&mut self, now: Instant) {
        self.start = Some(now);
        self.deadline = Some(now + Self::TIMEOUT);
        self.notified = false;
    }

    fn abort(&mut self) {
        self.start = None;
        self.deadline = None;
        self.notified = false;
    }

    fn has_expired(&self) -> bool {
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
        assert!(rx.try_recv().is_ok(), "buffered message should be flushed");
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
mod idle_tracker_tests {
    use super::*;
    use tokio::time::Instant;

    #[test]
    fn start_countdown_sets_deadline() {
        let mut tracker = IdleTracker::new();
        let now = Instant::now();
        tracker.start_countdown(now);
        assert!(tracker.deadline().is_some());
        assert!(!tracker.has_expired());
    }

    #[test]
    fn abort_clears_state() {
        let mut tracker = IdleTracker::new();
        tracker.start_countdown(Instant::now());
        tracker.abort();
        assert!(tracker.deadline().is_none());
        assert!(!tracker.has_expired());
    }

    #[test]
    fn fire_sets_notified() {
        let mut tracker = IdleTracker::new();
        let now = Instant::now();
        tracker.start_countdown(now);
        let start = tracker.fire(now);
        assert!(tracker.has_expired());
        assert!(tracker.deadline().is_none());
        assert_eq!(start, now);
    }

    #[test]
    fn fire_uses_fallback_when_start_is_none() {
        let mut tracker = IdleTracker::new();
        let now = Instant::now();
        let start = tracker.fire(now);
        assert_eq!(start, now);
        assert!(tracker.has_expired());
    }

    #[test]
    fn deadline_returns_none_after_fire() {
        let mut tracker = IdleTracker::new();
        tracker.start_countdown(Instant::now());
        tracker.fire(Instant::now());
        assert!(tracker.deadline().is_none());
    }
}
