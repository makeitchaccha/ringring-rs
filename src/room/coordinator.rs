use crate::room::coordinator::registry::MemberRegistry;
use crate::room::model::Room;
use crate::room::session::{
    SessionEvent, SessionHandle, SessionMessage, ShutdownReason, VoiceStateUpdate,
};
use crate::room::types::Moment;
use crate::room::{RoomId, RoomIdGenerator, Session};
use better_tokio_select::tokio_select;
use serenity::all::{ChannelId, GuildId, Member, UserId};
use std::collections::HashMap;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

/// A handle to communicate with the [`Coordinator`].
///
/// This handle allows tracking and notifying voice channel activities from other parts of the system,
/// such as the Discord event handler.
pub struct CoordinatorHandle {
    tx: mpsc::UnboundedSender<CoordinatorMessage>,
}

impl CoordinatorHandle {
    pub fn new(tx: mpsc::UnboundedSender<CoordinatorMessage>) -> Self {
        Self { tx }
    }

    /// Tracks an activity in a specific voice channel.
    ///
    /// If a session for the channel does not exist, the coordinator will spawn a new one.
    pub fn track(
        &self,
        channel_id: ChannelId,
        guild_id: GuildId,
        message: VoiceStateUpdate,
    ) -> Result<(), SendError<CoordinatorMessage>> {
        self.tx.send(CoordinatorMessage::Track {
            channel_id,
            guild_id,
            message,
        })
    }

    /// Notifies a session of an event without creating a new session if it doesn't exist.
    pub fn notify(
        &self,
        guild_id: GuildId,
        channel_id: Option<ChannelId>,
        message: VoiceStateUpdate,
    ) -> Result<(), SendError<CoordinatorMessage>> {
        self.tx.send(CoordinatorMessage::Notify {
            guild_id,
            channel_id,
            message,
        })
    }
}

pub enum CoordinatorEvent {
    /// Emitted when a new session is published and ready to be consumed (e.g., by a reporter).
    Published {
        channel_id: ChannelId,
        session_event_rx: broadcast::Receiver<SessionEvent>,
    },
}

pub enum CoordinatorMessage {
    /// Dispatch a message to a session, creating it if necessary.
    Track {
        guild_id: GuildId,
        channel_id: ChannelId,
        message: VoiceStateUpdate,
    },
    /// Dispatch a message to an existing session.
    Notify {
        guild_id: GuildId,
        channel_id: Option<ChannelId>,
        message: VoiceStateUpdate,
    },
}

/// Internal messages used by [`Session`](crate::room::Session) to communicate its lifecycle state back to the coordinator.
pub enum CoordinatorInternalMessage {
    /// Sent when a session has been empty for the timeout period.
    Idle {
        channel_id: ChannelId,
        since: Moment,
    },
    /// Sent by a session to confirm it has successfully shut down.
    AcceptShutdown { channel_id: ChannelId, room: Room },
    /// Sent by a session to reject a shutdown request (e.g., because someone re-joined).
    RejectShutdown { channel_id: ChannelId },
}

/// The central orchestrator for all voice channel sessions.
///
/// The `Coordinator` manages the lifecycle of [`Session`](crate::room::Session) instances,
/// maintaining one active session per voice channel. It acts as a router for incoming
/// Discord voice state events and ensures that sessions are properly created and disposed of.
///
/// ### Graceful Shutdown & Race Conditions
/// A key responsibility of the `Coordinator` is managing the "Idle -> Shutdown" transition.
/// To handle the race condition where a user joins a channel exactly when the session is
/// shutting down, the coordinator uses a suspension mechanism:
///
/// 1. When a session reports being [`Idle`](CoordinatorInternalMessage::Idle), the coordinator
///    instructs the [`SessionHandle`] to buffer any new incoming events.
/// 2. It then sends a shutdown request to the session.
/// 3. If the session confirms shutdown ([`AcceptShutdown`](CoordinatorInternalMessage::AcceptShutdown)),
///    the coordinator checks if any events were buffered during the wait.
/// 4. If events exist, it immediately spawns a *new* session and flushes the buffered events to it.
/// 5. If the session rejects shutdown ([`RejectShutdown`](CoordinatorInternalMessage::RejectShutdown)),
///    the coordinator simply resumes delivery of buffered events to the existing session.
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
        id: RoomId,
        guild_id: GuildId,
        channel_id: ChannelId,
        internal_tx: &mpsc::UnboundedSender<CoordinatorInternalMessage>,
    ) -> mpsc::UnboundedSender<SessionMessage> {
        let (session_event_tx, session_event_rx) = broadcast::channel(1);
        let room = Room::new(id, guild_id, channel_id, Moment::now());
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

        let mut id_generator = RoomIdGenerator::new();
        let mut registry = MemberRegistry::new();

        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();

        loop {
            tokio_select!(
                biased,
                match .. {
                    .. if let Some(message) = internal_rx.recv() => {
                        match message {
                            CoordinatorInternalMessage::Idle { channel_id, since } => {
                                let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                    warn!(
                                        "Coordinator received idle message from already disposed session, so just ignore the message."
                                    );
                                    continue;
                                };

                                if handle.is_suspended() {
                                    warn!(
                                        "Coordinator received idle message while waiting for session response, so just ignore the message."
                                    );
                                    continue;
                                }

                                info!("Started shutdown sequence for session:{}", channel_id);

                                // 1. Suspend normal event delivery to prevent race conditions during shutdown.
                                handle.suspend_delivery();

                                // 2. Force-dispatch a shutdown request. This bypasses the buffer we just started.
                                if let Err(err) =
                                    handle.force_dispatch(SessionMessage::RequestShutdown {
                                        reason: ShutdownReason::Idle,
                                        end: since,
                                    })
                                {
                                    warn!("could not request shutdown: {}", err);
                                    self.sessions.remove(&channel_id);
                                }
                            }
                            CoordinatorInternalMessage::AcceptShutdown { channel_id, room } => {
                                let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                    warn!(
                                        "Coordinator received accept-shutdown, so just ignore the message."
                                    );
                                    continue;
                                };

                                if handle.has_waiting_events() {
                                    info!(
                                        "Handle has waiting events. Creating new session and reconnecting handle..."
                                    );
                                    let tx = Self::spawn_session(
                                        &self.event_tx,
                                        id_generator.next_id(),
                                        room.guild_id,
                                        room.channel_id,
                                        &internal_tx,
                                    );
                                    handle.reconnect(tx);
                                    if let Err(err) = handle.resume_delivery() {
                                        error!("failed to resume handle: {}", err);
                                        self.sessions.remove(&channel_id);
                                    }
                                } else {
                                    self.sessions.remove(&channel_id);
                                    registry.drain_channel(room.guild_id, channel_id);
                                }
                            }

                            CoordinatorInternalMessage::RejectShutdown { channel_id } => {
                                let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                    error!(
                                        "Coordinator received reject-shutdown from already disposed session, so just ignore the message."
                                    );
                                    continue;
                                };

                                if let Err(err) = handle.resume_delivery() {
                                    warn!("could not resume delivery: {}", err);
                                    self.sessions.remove(&channel_id);
                                }
                            }
                        }
                    }

                    .. if let Some(message) = self.rx.recv() => {
                        match message {
                            CoordinatorMessage::Track {
                                channel_id,
                                guild_id,
                                message,
                            } => {
                                if let Some(user_location) =
                                    registry.get_channel(guild_id, message.user_id)
                                    && user_location != channel_id
                                    && let Some(handle) = self.sessions.get_mut(&user_location)
                                    && let Err(error) = handle.dispatch(
                                        SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
                                            now: message.now,
                                            user_id: message.user_id,
                                            flags: None,
                                        }),
                                    )
                                {
                                    error!(?error, "could not dispatch message");
                                    self.sessions.remove(&user_location);
                                }

                                let handle = self.sessions.entry(channel_id).or_insert_with(|| {
                                    let tx = Self::spawn_session(
                                        &self.event_tx,
                                        id_generator.next_id(),
                                        guild_id,
                                        channel_id,
                                        &internal_tx,
                                    );

                                    SessionHandle::new(tx)
                                });

                                let user_id = message.user_id;
                                if let Err(error) =
                                    handle.dispatch(SessionMessage::VoiceStateUpdate(message))
                                {
                                    error!(?error, "could not dispatch message");
                                    self.sessions.remove(&channel_id);
                                }
                                registry.bind(guild_id, user_id, channel_id);
                            }
                            CoordinatorMessage::Notify {
                                guild_id,
                                channel_id,
                                message,
                            } => {
                                if let Some(user_location) =
                                    registry.get_channel(guild_id, message.user_id)
                                    && channel_id
                                        .is_none_or(|channel_id| channel_id != user_location)
                                    && let Some(handle) = self.sessions.get_mut(&user_location)
                                    && let Err(error) = handle.dispatch(
                                        SessionMessage::VoiceStateUpdate(VoiceStateUpdate {
                                            now: message.now,
                                            user_id: message.user_id,
                                            flags: None,
                                        }),
                                    )
                                {
                                    error!(?error, "could not dispatch message");
                                    self.sessions.remove(&user_location);
                                    registry.unbind(guild_id, message.user_id);
                                };

                                let Some(channel_id) = channel_id else {
                                    // just ignore
                                    continue;
                                };

                                let Some(handle) = self.sessions.get_mut(&channel_id) else {
                                    // just ignore
                                    continue;
                                };

                                let user_id = message.user_id;
                                if let Err(err) =
                                    handle.dispatch(SessionMessage::VoiceStateUpdate(message))
                                {
                                    error!("could not dispatch message: {}", err);
                                    self.sessions.remove(&channel_id);
                                }
                                registry.bind(guild_id, user_id, channel_id);
                            }
                        }
                    }
                }
            );
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

mod registry {
    use super::*;
    pub(super) struct MemberRegistry {
        member_channel: HashMap<(GuildId, UserId), ChannelId>,
        channel_members: HashMap<(GuildId, ChannelId), Vec<UserId>>,
    }

    impl MemberRegistry {
        pub(super) fn new() -> Self {
            Self {
                member_channel: HashMap::new(),
                channel_members: HashMap::new(),
            }
        }

        pub(super) fn unbind(&mut self, guild: GuildId, user: UserId) -> Option<ChannelId> {
            let channel = self.member_channel.remove(&(guild, user))?;
            let members = self
                .channel_members
                .get_mut(&(guild, channel))
                .expect("must exist");

            if let Some(pos) = members.iter().position(|&m| m == user) {
                members.swap_remove(pos);
            }
            if members.is_empty() {
                self.channel_members
                    .remove(&(guild, channel))
                    .expect("must exist");
            }

            Some(channel)
        }

        pub(super) fn bind(&mut self, guild: GuildId, user: UserId, channel: ChannelId) {
            if self.get_channel(guild, user) == Some(channel) {
                return;
            }
            self.unbind(guild, user);
            self.member_channel.insert((guild, user), channel);
            self.channel_members
                .entry((guild, channel))
                .or_default()
                .push(user);
        }

        pub(super) fn drain_channel(&mut self, guild: GuildId, channel: ChannelId) -> Vec<UserId> {
            let Some(members) = self.channel_members.remove(&(guild, channel)) else {
                return vec![];
            };
            for &user in members.iter() {
                self.member_channel
                    .remove(&(guild, user))
                    .expect("must exist");
            }

            members
        }

        pub(super) fn get_channel(&self, guild: GuildId, user: UserId) -> Option<ChannelId> {
            self.member_channel.get(&(guild, user)).copied()
        }
    }
}
