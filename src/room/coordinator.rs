use crate::room::RoomActor;
use crate::room::actor::{RoomHandle, RoomMessage, ShutdownReason};
use crate::room::model::Room;
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
    rooms: HashMap<ChannelId, RoomHandle>,
}

impl Coordinator {
    fn new() -> Self {
        Self {
            rooms: HashMap::new(),
        }
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
                            let Some(handle) = self.rooms.get_mut(&channel_id) else {
                                warn!("Coordinator received idle message from already disposed room, so just ignore the message.");
                                continue;
                            };

                            if handle.is_suspended() {
                                warn!("Coordinator received idle message while waiting for room response, so just ignore the message.");
                                continue;
                            }

                            info!("Started shutdown sequence for room:{}", channel_id);
                            handle.suspend_delivery();
                            if let Err(err) = handle.bypass(RoomMessage::RequestShutdown {
                                reason: ShutdownReason::Idle,
                            }) {
                                warn!("could not request shutdown: {}", err);
                                self.rooms.remove(&channel_id);
                            }
                        }
                        CoordinatorInternalMessage::AcceptShutdown { channel_id, room } => {
                            let Some(handle) = self.rooms.get_mut(&channel_id) else {
                                warn!("Coordinator received accept-shutdown, so just ignore the message.");
                                continue;
                            };

                            if handle.has_suspended_events() {
                                info!("Handle has suspended events. Creating new room and reconnecting handle...");
                                let room = Room::new(room.guild_id, room.channel_id, Moment::now());
                                let tx = start_room_actor(room, internal_tx.clone());
                                handle.reconnect(tx);
                            } else {
                                self.rooms.remove(&channel_id);
                            }
                        }
                        CoordinatorInternalMessage::RejectShutdown { channel_id } => {
                            let Some(handle) = self.rooms.get_mut(&channel_id) else {
                                error!("Coordinator received reject-shutdown from already disposed room, so just ignore the message.");
                                continue;
                            };

                            if let Err(err) = handle.resume_delivery() {
                                warn!("could not resume delivery: {}", err);
                                self.rooms.remove(&channel_id);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn start_room_actor(
    room: Room,
    internal_tx: mpsc::UnboundedSender<CoordinatorInternalMessage>,
) -> mpsc::UnboundedSender<RoomMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    let actor = RoomActor::new(room, rx, internal_tx);

    tokio::spawn(actor.run());

    tx
}
