use crate::room::coordinator::CoordinatorInternalMessage;
use crate::room::model::{Room, RoomStatus};
use crate::room::types::VoiceStateFlags;
use crate::room::{Moment, RoomLease};
use serenity::all::UserId;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use better_tokio_select::tokio_select;
use tokio::select;
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
    /// The session's internal state (room model) has been updated.
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

        let mut idle_timer = IdleTimer::with_timeout(Duration::from_secs(60));

        loop {
            tokio_select!(biased, match .. {
                .. if let Some(cmd) = self.rx.recv() => {
                    match cmd {
                        SessionMessage::VoiceStateUpdate(voice_state_update) => {
                            let Some(flags) = voice_state_update.flags else {
                                info!(user_id = %voice_state_update.user_id, "Participant disconnected");
                                let status = match self.room.handle_disconnect(voice_state_update.now, voice_state_update.user_id){
                                    Ok(status) => status,
                                    Err(error) => {
                                        warn!(?error, "room inconsistency was detected. just ignored the update.");
                                        continue;
                                    }
                                };
                                if status == RoomStatus::Empty {
                                    info!("Room is now empty, starting idle countdown");
                                    idle_timer.start_countdown();
                                }
                                let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });

                                continue;
                            };

                            if self.room.is_connected(voice_state_update.user_id) {
                                info!(user_id = %voice_state_update.user_id, flags = ?voice_state_update.flags, "Participant state updated");
                                self.room.handle_update(voice_state_update.now, voice_state_update.user_id, flags).expect("invalid state");
                                let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });
                            } else {
                                info!(user_id = %voice_state_update.user_id, "Participant connected");
                                self.room.handle_connect(voice_state_update.now, voice_state_update.user_id, flags).expect("invalid state");
                                idle_timer.abort();
                                let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });
                            }
                        }
                        SessionMessage::RequestShutdown{ reason, end } => {
                            if !self.room.is_empty() {
                                warn!(?reason, "Shutdown requested but room is not empty. Rejecting.");
                                if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::RejectShutdown {
                                    channel_id: self.room.channel_id
                                }) {
                                    error!(error = %err, "Failed to send shutdown rejection");
                                    let _ = self.event_tx.send(SessionEvent::Shutdown { room: self.room.lease(), end });
                                    break;
                                }
                                continue;
                            }

                            if reason == ShutdownReason::Idle && !idle_timer.has_expired(Instant::now()) {
                                info!("Shutdown requested but room is recently active. Rejecting.");
                                if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::RejectShutdown {
                                    channel_id: self.room.channel_id
                                }) {
                                    error!(error = %err, "Failed to send shutdown rejection");
                                    let _ = self.event_tx.send(SessionEvent::Shutdown { room: self.room.lease(), end });
                                    break;
                                }
                                continue;
                            }

                            let _ = self.event_tx.send(SessionEvent::Shutdown { room: self.room.lease(), end });
                            if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::AcceptShutdown { channel_id: self.room.channel_id, room: self.room }) {
                                error!(error = %err, "Failed to send shutdown acceptance");
                            }

                            info!("Session stopped");
                            break;
                        }
                    }
                }

                .. if let _ = &mut idle_timer => {
                    idle_timer.wait_for_shutdown_request();
                    if let Some(status) = idle_timer.status.as_ref() {
                        info!(idle_since = ?status.start, "Detected idle, starting idle-shutdown sequence...");
                        if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::Idle { channel_id: self.room.channel_id, since: self.room.start.at(status.start) }) {
                            error!(error = %err, "Failed to send idle notification");
                            break;
                        }
                    }else {
                        info!("Safety timer fired for long-running session; refreshing timer");
                        idle_timer.abort();
                    }
                }
            })
        }
    }
}

struct IdleTimer {
    inner: Pin<Box<tokio::time::Sleep>>,
    timeout: Duration,
    status: Option<IdleStatus>,
}

struct IdleStatus {
    start: Instant,
    deadline: Instant,
}

impl IdleTimer {
    const FAR_FUTURE: Duration = Duration::from_secs(86400 * 30);

    fn with_timeout(timeout: Duration) -> Self {
        Self {
            inner: Box::pin(tokio::time::sleep(IdleTimer::FAR_FUTURE)),
            timeout,
            status: None,
        }
    }

    fn start_countdown(&mut self) {
        let start = Instant::now();
        let deadline = start + self.timeout;
        self.inner.as_mut().reset(deadline);
        self.status = Some(IdleStatus { start, deadline });
    }

    fn has_expired(&self, now: Instant) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.deadline < now)
    }

    fn wait_for_shutdown_request(&mut self) {
        self.inner.as_mut().reset(Instant::now() + Self::FAR_FUTURE);
    }

    fn abort(&mut self) {
        self.inner.as_mut().reset(Instant::now() + Self::FAR_FUTURE);
        self.status = None;
    }
}

impl Future for IdleTimer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().inner.as_mut().poll(cx)
    }
}
