use crate::room::coordinator::CoordinatorInternalMessage;
use crate::room::model::{Room, RoomStatus};
use crate::room::types::{UserIdentity, VoiceStateFlags};
use serenity::all::UserId;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tokio::time::Instant;
use tracing::{error, info};

pub struct SessionHandle {
    suspended_events: Option<Vec<SessionMessage>>,
    tx: mpsc::Sender<SessionMessage>,
}

impl SessionHandle {
    pub fn new(tx: mpsc::Sender<SessionMessage>) -> Self {
        Self {
            tx,
            suspended_events: None,
        }
    }

    pub async fn dispatch_or_hold(
        &mut self,
        event: SessionMessage,
    ) -> Result<(), SendError<SessionMessage>> {
        match self.suspended_events.as_mut() {
            Some(queue) => queue.push(event),
            None => self.tx.send(event).await?,
        }
        Ok(())
    }

    pub async fn bypass(&self, event: SessionMessage) -> Result<(), SendError<SessionMessage>> {
        self.tx.send(event).await?;
        Ok(())
    }

    pub fn suspend_delivery(&mut self) {
        if self.suspended_events.is_none() {
            self.suspended_events = Some(Vec::new());
        }
    }

    pub async fn resume_delivery(&mut self) -> Result<(), SendError<SessionMessage>> {
        if let Some(queue) = self.suspended_events.take() {
            for event in queue {
                self.tx.send(event).await?;
            }
        }
        Ok(())
    }

    pub fn reconnect(&mut self, new_rx: mpsc::Sender<SessionMessage>) {
        self.tx = new_rx;
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended_events.is_some()
    }

    pub fn has_suspended_events(&self) -> bool {
        self.suspended_events
            .as_ref()
            .is_some_and(|events| events.len() > 0)
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
        identification: UserIdentity,
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
    },
}

pub struct Session {
    room: Room,
    rx: mpsc::Receiver<SessionMessage>,
    coordinator_tx: mpsc::Sender<CoordinatorInternalMessage>,
}

impl Session {
    pub fn new(
        room: Room,
        rx: mpsc::Receiver<SessionMessage>,
        coordinator_tx: mpsc::Sender<CoordinatorInternalMessage>,
    ) -> Session {
        Session {
            room,
            rx,
            coordinator_tx,
        }
    }

    pub async fn run(mut self) {
        info!("Room session started");

        let mut idle_timer = IdleTimer::with_timeout(Duration::from_secs(60));

        loop {
            select! {
                biased;

                Some(cmd) = self.rx.recv() => {
                    match cmd {
                        SessionMessage::Connect{ now, identification, flags } => {
                            self.room.handle_connect(now, identification, flags).expect("invalid state");
                            idle_timer.abort();
                        }
                        SessionMessage::Disconnect{ now, user_id } => {
                            let status = self.room.handle_disconnect(now, user_id).expect("invalid state");
                            if status == RoomStatus::Empty {
                                idle_timer.start_countdown();
                            }
                        }
                        SessionMessage::Update{ now, user_id, flags } => {
                            self.room.handle_update(now, user_id, flags).expect("invalid state");
                        }

                        SessionMessage::RequestShutdown{ reason } => {
                            if !self.room.is_empty() {
                                info!("Session has requested to shutdown but is not empty. Reject shutdown.");
                                if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::RejectShutdown {
                                    channel_id: self.room.channel_id
                                }).await {
                                    error!("failed to send shutdown rejection to coordinator: {}", err);
                                    break;
                                }
                                continue;
                            }

                            if reason == ShutdownReason::Idle && !idle_timer.has_expired(Instant::now()) {
                                info!("Session has requested to shutdown but is recently participant joined and then left. Reject shutdown.");
                                if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::RejectShutdown {
                                    channel_id: self.room.channel_id
                                }).await {
                                    error!("failed to send shutdown rejection to coordinator: {}", err);
                                    break;
                                }
                                continue;
                            }

                            if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::AcceptShutdown { channel_id: self.room.channel_id, room: self.room }).await {
                                error!("failed to send shutdown ready to coordinator: {}", err);
                            }

                            info!("Session is stopping...");
                            break;
                        }
                    }
                }

                _ = &mut idle_timer => {
                    info!("detected idle, starting idle-shutdown sequence...");
                    idle_timer.wait_for_shutdown_request();
                    if let Err(err) = self.coordinator_tx.send(CoordinatorInternalMessage::Idle { channel_id: self.room.channel_id }).await {
                        error!("failed to send idle notification to coordinator: {}", err);
                        break;
                    }
                }
            }
        }
    }
}

struct IdleTimer {
    inner: Pin<Box<tokio::time::Sleep>>,
    timeout: Duration,
    expire_at: Option<Instant>,
}

impl IdleTimer {
    fn with_timeout(timeout: Duration) -> Self {
        Self {
            inner: Box::pin(tokio::time::sleep(Duration::MAX)),
            timeout,
            expire_at: None,
        }
    }

    fn start_countdown(&mut self) {
        let deadline = Instant::now() + self.timeout;
        self.inner.as_mut().reset(deadline);
        self.expire_at = Some(deadline);
    }

    fn has_expired(&self, now: Instant) -> bool {
        self.expire_at.is_some_and(|expire_at| expire_at < now)
    }

    fn wait_for_shutdown_request(&mut self) {
        self.inner.as_mut().reset(Instant::now() + Duration::MAX);
    }

    fn abort(&mut self) {
        self.inner.as_mut().reset(Instant::now() + Duration::MAX);
        self.expire_at = None;
    }
}

impl std::future::Future for IdleTimer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().inner.as_mut().poll(cx)
    }
}
