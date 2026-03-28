use crate::room::Session;
use crate::room::model::Room;
use crate::room::session::{SessionEvent, SessionHandle, SessionMessage, ShutdownReason};
use crate::room::types::Moment;
use serenity::all::{ChannelId, GuildId};
use std::collections::HashMap;
use tokio::select;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

pub struct CoordinatorHandle {
    tx: mpsc::UnboundedSender<CoordinatorMessage>,
}

impl CoordinatorHandle {
    pub fn new(tx: mpsc::UnboundedSender<CoordinatorMessage>) -> Self {
        Self { tx }
    }

    pub fn track(
        &self,
        channel_id: ChannelId,
        guild_id: GuildId,
        message: SessionMessage,
    ) -> Result<(), SendError<CoordinatorMessage>> {
        self.tx.send(CoordinatorMessage::Track {
            channel_id,
            guild_id,
            message,
        })
    }

    pub fn notify(
        &self,
        channel_id: ChannelId,
        message: SessionMessage,
    ) -> Result<(), SendError<CoordinatorMessage>> {
        self.tx.send(CoordinatorMessage::Notify {
            channel_id,
            message,
        })
    }
}

pub enum CoordinatorEvent {
    Published {
        channel_id: ChannelId,
        session_event_rx: broadcast::Receiver<SessionEvent>,
    },
}

pub enum CoordinatorMessage {
    Track {
        channel_id: ChannelId,
        guild_id: GuildId,
        message: SessionMessage,
    },
    Notify {
        channel_id: ChannelId,
        message: SessionMessage,
    },
}

pub enum CoordinatorInternalMessage {
    Idle {
        channel_id: ChannelId,
        since: Moment,
    },
    AcceptShutdown {
        channel_id: ChannelId,
        room: Room,
    },
    RejectShutdown {
        channel_id: ChannelId,
    },
}

pub struct Coordinator {
    sessions: HashMap<ChannelId, SessionHandle>,
    rx: mpsc::UnboundedReceiver<CoordinatorMessage>,
    event_tx: mpsc::UnboundedSender<CoordinatorEvent>,
}

impl Coordinator {
    pub fn new(
        rx: mpsc::UnboundedReceiver<CoordinatorMessage>,
        event_tx: mpsc::UnboundedSender<CoordinatorEvent>,
    ) -> Self {
        Self {
            sessions: HashMap::new(),
            rx,
            event_tx,
        }
    }

    pub fn spawn() -> (CoordinatorHandle, mpsc::UnboundedReceiver<CoordinatorEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let coordinator = Self::new(rx, event_tx);

        tokio::spawn(coordinator.run());

        (CoordinatorHandle::new(tx), event_rx)
    }

    fn spawn_session(
        event_tx: &mpsc::UnboundedSender<CoordinatorEvent>,
        guild_id: GuildId,
        channel_id: ChannelId,
        internal_tx: &mpsc::UnboundedSender<CoordinatorInternalMessage>,
    ) -> mpsc::UnboundedSender<SessionMessage> {
        let (session_event_tx, session_event_rx) = broadcast::channel(1);
        let room = Room::new(guild_id, channel_id, Moment::now());
        let tx = start_session(room, internal_tx.clone(), session_event_tx);

        if let Err(err) = event_tx.send(CoordinatorEvent::Published {
            channel_id,
            session_event_rx,
        }) {
            warn!(
                "could not publish session for channel {}: {}",
                channel_id, err
            );
        }

        tx
    }

    pub async fn run(mut self) {
        info!("Starting coordinator loop");

        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();

        loop {
            select! {
                biased;
                Some(message) = internal_rx.recv() => {
                    match message {
                        CoordinatorInternalMessage::Idle { channel_id, since } => {
                            let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                warn!("Coordinator received idle message from already disposed session, so just ignore the message.");
                                continue;
                            };

                            if handle.is_suspended() {
                                warn!("Coordinator received idle message while waiting for session response, so just ignore the message.");
                                continue;
                            }

                            info!("Started shutdown sequence for session:{}", channel_id);
                            handle.suspend_delivery();
                            if let Err(err) = handle.bypass(SessionMessage::RequestShutdown {
                                reason: ShutdownReason::Idle,
                                end: since,
                            }) {
                                warn!("could not request shutdown: {}", err);
                                self.sessions.remove(&channel_id);
                            }
                        }
                        CoordinatorInternalMessage::AcceptShutdown { channel_id, room } => {
                            let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                warn!("Coordinator received accept-shutdown, so just ignore the message.");
                                continue;
                            };

                            if handle.has_suspended_events() {
                                info!("Handle has suspended events. Creating new session and reconnecting handle...");
                                let tx = Self::spawn_session(&self.event_tx, room.guild_id, room.channel_id, &internal_tx);
                                handle.reconnect(tx);
                                if let Err(err) = handle.resume_delivery() {
                                    error!("failed to resume handle: {}", err);
                                    self.sessions.remove(&channel_id);
                                }
                            } else {
                                self.sessions.remove(&channel_id);
                            }
                        }
                        CoordinatorInternalMessage::RejectShutdown { channel_id } => {
                            let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                error!("Coordinator received reject-shutdown from already disposed session, so just ignore the message.");
                                continue;
                            };

                            if let Err(err) = handle.resume_delivery() {
                                warn!("could not resume delivery: {}", err);
                                self.sessions.remove(&channel_id);
                            }
                        }
                    }
                }


                Some(message) = self.rx.recv() => {
                    match message {
                        CoordinatorMessage::Track { channel_id, guild_id, message } => {
                            let handle = self.sessions.entry(channel_id).or_insert_with(|| {
                                let tx = Self::spawn_session(&self.event_tx, guild_id, channel_id, &internal_tx);

                                SessionHandle::new(tx)
                            });

                            if let Err(err) = handle.dispatch_or_hold(message) {
                                error!("could not dispatch message: {}", err);
                                self.sessions.remove(&channel_id);
                            }
                        },
                        CoordinatorMessage::Notify { channel_id, message } => {
                            let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                // just ignore
                                continue;
                            };

                            if let Err(err) = handle.dispatch_or_hold(message) {
                                error!("could not dispatch message: {}", err);
                                self.sessions.remove(&channel_id);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn start_session(
    room: Room,
    internal_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
    event_tx: broadcast::Sender<SessionEvent>,
) -> mpsc::UnboundedSender<SessionMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    let session = Session::new(room, rx, internal_tx, event_tx);

    tokio::spawn(session.run());

    tx
}
