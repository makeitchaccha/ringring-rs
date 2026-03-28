use crate::reporting::subscription::SubscriptionProvider;
use crate::room::{CoordinatorHandle, SessionMessage, UserIdentity};
use serenity::all::{ChannelId, Context, EventHandler, GuildId, Http, VoiceState};
use serenity::async_trait;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, error, warn};

pub struct VoiceHandler {
    subscription_provider: Arc<dyn SubscriptionProvider>,
    coordinator_handle: CoordinatorHandle,
}

impl VoiceHandler {
    pub fn new(
        subscription_provider: Arc<dyn SubscriptionProvider>,
        coordinator_handle: CoordinatorHandle,
    ) -> Self {
        Self {
            subscription_provider,
            coordinator_handle,
        }
    }
}

impl VoiceHandler {
    async fn handle_connection(
        &self,
        http: &Http,
        now: Instant,
        channel_id: ChannelId,
        voice_state: &VoiceState,
    ) {
        let Some(guild_id) = voice_state.guild_id else {
            warn!("failed to get guild id");
            return;
        };

        let Ok(member) = guild_id.member(&http, voice_state.user_id).await else {
            warn!("failed to get member");
            return;
        };

        let message = SessionMessage::Connect {
            now,
            identity: UserIdentity {
                user_id: voice_state.user_id,
                name: Arc::from(member.display_name()),
                face: Arc::from(member.face()),
            },
            flags: voice_state.into(),
        };
        if self.subscription_provider.has_subscription(channel_id) {
            if let Err(err) = self.coordinator_handle.track(channel_id, guild_id, message) {
                error!("failed to send message to coordinator: {}", err);
            }
        } else if let Err(err) = self.coordinator_handle.notify(channel_id, message) {
            error!("failed to send message to coordinator: {}", err);
        }
    }

    fn handle_disconnection(&self, now: Instant, channel_id: ChannelId, voice_state: &VoiceState) {
        if let Err(err) = self.coordinator_handle.notify(
            channel_id,
            SessionMessage::Disconnect {
                now,
                user_id: voice_state.user_id,
            },
        ) {
            error!("failed to send message to coordinator: {}", err);
        }
    }

    fn handle_update(&self, now: Instant, channel_id: ChannelId, voice_state: &VoiceState) {
        if let Err(err) = self.coordinator_handle.notify(
            channel_id,
            SessionMessage::Update {
                now,
                user_id: voice_state.user_id,
                flags: voice_state.into(),
            },
        ) {
            error!("failed to send message to coordinator: {}", err);
        }
    }
}

#[async_trait]
impl EventHandler for VoiceHandler {
    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        debug!("cache is ready for guilds: {:?}", guilds);
        let now = Instant::now();
        let mut voice_states = Vec::new();

        for guild_id in guilds {
            let Some(guild_ref) = guild_id.to_guild_cached(&ctx) else {
                continue;
            };
            for voice_state in guild_ref.voice_states.values() {
                let mut voice_state = voice_state.clone();
                voice_state.guild_id = Some(guild_id);
                voice_states.push(voice_state);
            }
        }

        for voice_state in voice_states {
            if let Some(channel) = voice_state.channel_id {
                self.handle_connection(&ctx.http, now, channel, &voice_state)
                    .await;
            }
        }
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        let channel_activity = ChannelActivity::from_voice_states(&old, &new);

        match channel_activity {
            ChannelActivity::Connect { new_channel_id } => {
                self.handle_connection(&ctx.http, Instant::now(), new_channel_id, &new)
                    .await;
            }
            ChannelActivity::Disconnect { old_channel_id } => {
                self.handle_disconnection(Instant::now(), old_channel_id, &old.unwrap());
            }
            ChannelActivity::Move {
                old_channel_id,
                new_channel_id,
            } => {
                let now = Instant::now();
                self.handle_disconnection(now, old_channel_id, &old.unwrap());
                self.handle_connection(&ctx.http, now, new_channel_id, &new)
                    .await;
            }
            ChannelActivity::Update { channel_id } => {
                self.handle_update(Instant::now(), channel_id, &new);
            }
            ChannelActivity::Ignore => {}
        }
    }
}

enum ChannelActivity {
    Connect {
        new_channel_id: ChannelId,
    },
    Disconnect {
        old_channel_id: ChannelId,
    },
    Move {
        new_channel_id: ChannelId,
        old_channel_id: ChannelId,
    },
    Update {
        channel_id: ChannelId,
    },
    Ignore,
}

impl ChannelActivity {
    fn from_voice_states(old: &Option<VoiceState>, new: &VoiceState) -> Self {
        match (old.as_ref().and_then(|v| v.channel_id), new.channel_id) {
            (Some(old_channel_id), None) => ChannelActivity::Disconnect { old_channel_id },
            (None, Some(new_channel_id)) => ChannelActivity::Connect { new_channel_id },
            (Some(old_channel_id), Some(new_channel_id)) => {
                if old_channel_id != new_channel_id {
                    ChannelActivity::Move {
                        old_channel_id,
                        new_channel_id,
                    }
                } else {
                    ChannelActivity::Update {
                        channel_id: new_channel_id,
                    }
                }
            }
            _ => ChannelActivity::Ignore,
        }
    }
}
