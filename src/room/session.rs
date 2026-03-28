use crate::room::coordinator::CoordinatorInternalMessage;
use crate::room::model::{Room, RoomStatus};
use crate::room::types::{UserIdentity, VoiceStateFlags};
use crate::room::{Moment, RoomLease};
use serenity::all::UserId;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

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

    pub fn dispatch_or_hold(
        &mut self,
        event: SessionMessage,
    ) -> Result<(), SendError<SessionMessage>> {
        match self.suspended_events.as_mut() {
            Some(queue) => queue.push(event),
            None => self.tx.send(event)?,
        }
        Ok(())
    }

    pub fn bypass(&self, event: SessionMessage) -> Result<(), SendError<SessionMessage>> {
        self.tx.send(event)?;
        Ok(())
    }

    pub fn suspend_delivery(&mut self) {
        if self.suspended_events.is_none() {
            self.suspended_events = Some(Vec::new());
        }
    }

    pub fn resume_delivery(&mut self) -> Result<(), SendError<SessionMessage>> {
        if let Some(queue) = self.suspended_events.take() {
            for event in queue {
                self.tx.send(event)?;
            }
        }
        Ok(())
    }

    pub fn reconnect(&mut self, new_tx: mpsc::UnboundedSender<SessionMessage>) {
        self.tx = new_tx;
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended_events.is_some()
    }

    pub fn has_suspended_events(&self) -> bool {
        self.suspended_events
            .as_ref()
            .is_some_and(|events| !events.is_empty())
    }
}

#[derive(Debug, PartialEq)]
pub enum ShutdownReason {
    Idle,
    External,
}

pub enum SessionMessage {
    Connect {
        now: Instant,
        identity: UserIdentity,
        flags: VoiceStateFlags,
    },
    Disconnect {
        now: Instant,
        user_id: UserId,
    },
    Update {
        now: Instant,
        user_id: UserId,
        flags: VoiceStateFlags,
    },
    RequestShutdown {
        reason: ShutdownReason,
        end: Moment,
    },
}

#[derive(Clone)]
pub enum SessionEvent {
    Updated { room: RoomLease },
    Shutdown { room: RoomLease, end: Moment },
}

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
            select! {
                biased;

                Some(cmd) = self.rx.recv() => {
                    match cmd {
                        SessionMessage::Connect{ now, identity, flags } => {
                            info!(user = %identity.name, user_id = %identity.user_id, "Participant connected");
                            self.room.handle_connect(now, identity, flags).expect("invalid state");
                            idle_timer.abort();
                            let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });
                        }
                        SessionMessage::Disconnect{ now, user_id } => {
                            info!(%user_id, "Participant disconnected");
                            let status = self.room.handle_disconnect(now, user_id).expect("invalid state");
                            if status == RoomStatus::Empty {
                                info!("Room is now empty, starting idle countdown");
                                idle_timer.start_countdown();
                            }
                            let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });
                        }
                        SessionMessage::Update{ now, user_id, flags } => {
                            debug!(%user_id, ?flags, "Participant state updated");
                            self.room.handle_update(now, user_id, flags).expect("invalid state");
                            let _ = self.event_tx.send(SessionEvent::Updated { room: self.room.lease() });
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

                _ = &mut idle_timer => {

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
            }
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
