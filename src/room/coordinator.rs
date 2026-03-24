use crate::room::Session;
use crate::room::model::Room;
use crate::room::session::{SessionHandle, SessionMessage, ShutdownReason};
use crate::room::types::Moment;
use serenity::all::ChannelId;
use std::collections::HashMap;
use tokio::select;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

pub enum CoordinatorInternalMessage {
    Idle { channel_id: ChannelId },
    AcceptShutdown { channel_id: ChannelId, room: Room },
    RejectShutdown { channel_id: ChannelId },
}

pub struct Coordinator {
    sessions: HashMap<ChannelId, SessionHandle>,
}

impl Coordinator {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub async fn run(mut self) {
        info!("Starting coordinator loop");

        let (internal_tx, mut internal_rx) = mpsc::channel(128);

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
                            }).await {
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
                            } else {
                                self.sessions.remove(&channel_id);
                            }
                        }
                        CoordinatorInternalMessage::RejectShutdown { channel_id } => {
                            let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                error!("Coordinator received reject-shutdown from already disposed session, so just ignore the message.");
                                continue;
                            };

                            if let Err(err) = handle.resume_delivery().await {
                                warn!("could not resume delivery: {}", err);
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
    internal_tx: mpsc::Sender<CoordinatorInternalMessage>,
) -> mpsc::Sender<SessionMessage> {
    let (tx, rx) = mpsc::channel(128);
    let session = Session::new(room, rx, internal_tx);

    tokio::spawn(session.run());

    tx
}
