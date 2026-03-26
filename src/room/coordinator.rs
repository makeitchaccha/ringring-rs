use crate::room::Session;
use crate::room::model::Room;
use crate::room::session::{SessionHandle, SessionMessage, ShutdownReason};
use crate::room::types::Moment;
use serenity::all::{ChannelId, GuildId};
use std::collections::HashMap;
use tokio::select;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tracing::{error, info, warn};

pub struct CoordinatorHandle {
    tx: mpsc::UnboundedSender<CoordinatorMessage>,
}

impl CoordinatorHandle {
    pub fn new(tx: mpsc::UnboundedSender<CoordinatorMessage>) -> Self {
        Self { tx }
    }

    pub fn track(&self, channel_id: ChannelId, guild_id: GuildId, message: SessionMessage) -> Result<(), SendError<CoordinatorMessage>> {
        self.tx.send(CoordinatorMessage::Track { channel_id, guild_id, message })
    }

    pub fn notify(&self, channel_id: ChannelId, message: SessionMessage) -> Result<(), SendError<CoordinatorMessage>>{
        self.tx.send(CoordinatorMessage::Notify { channel_id, message })
    }
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
    Idle { channel_id: ChannelId },
    AcceptShutdown { channel_id: ChannelId, room: Room },
    RejectShutdown { channel_id: ChannelId },
}

pub struct Coordinator {
    sessions: HashMap<ChannelId, SessionHandle>,
    rx: mpsc::UnboundedReceiver<CoordinatorMessage>,
}

impl Coordinator {
    pub fn new(rx: mpsc::UnboundedReceiver<CoordinatorMessage>) -> Self {
        Self {
            sessions: HashMap::new(),
            rx,
        }
    }

    pub fn spawn() -> CoordinatorHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let coordinator = Self::new(rx);

        tokio::spawn(coordinator.run());

        CoordinatorHandle::new(tx)
    }

    pub async fn run(mut self) {
        info!("Starting coordinator loop");

        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();

        loop {
            select! {
                biased;
                Some(message) = internal_rx.recv() => {
                    match message {
                        CoordinatorInternalMessage::Idle { channel_id } => {
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
                                let room = Room::new(room.guild_id, room.channel_id, Moment::now());
                                let tx = start_session(room, internal_tx.clone());
                                handle.reconnect(tx);
                                if let Err(err) = handle.resume_delivery() {
                                    error!("failed to resume session: {}", err);
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
                                let room = Room::new(guild_id, channel_id, Moment::now());
                                let tx = start_session(room, internal_tx.clone());
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
) -> mpsc::UnboundedSender<SessionMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    let session = Session::new(room, rx, internal_tx);

    tokio::spawn(session.run());

    tx
}
