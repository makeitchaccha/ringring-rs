use crate::reporting::subscription::SubscriptionProvider;
use crate::room;
use crate::room::CoordinatorHandle;
use serenity::all::{Context, EventHandler, FullEvent};
use serenity::async_trait;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, error};

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

#[async_trait]
impl EventHandler for VoiceHandler {
    async fn dispatch(&self, ctx: &Context, event: &FullEvent) {
        match event {
            FullEvent::CacheReady { guilds, .. } => {
                debug!("cache is ready for guilds: {:?}", guilds);
                let now = Instant::now();
                let mut voice_states = Vec::new();

                for &guild_id in guilds {
                    let Some(guild_ref) = guild_id.to_guild_cached(&ctx.cache) else {
                        continue;
                    };

                    for voice_state in guild_ref.voice_states.iter() {
                        if !voice_state.channel_id.is_some_and(|channel_id| {
                            self.subscription_provider.has_subscription(channel_id)
                        }) {
                            continue;
                        }
                        let mut voice_state = voice_state.clone();
                        voice_state.guild_id = Some(guild_id);
                        voice_states.push(voice_state);
                    }
                }

                for voice_state in &voice_states {
                    if let Some(channel) = voice_state.channel_id {
                        if let Err(error) = self.coordinator_handle.track(
                            channel,
                            voice_state.guild_id.unwrap(),
                            room::VoiceStateUpdate {
                                now,
                                user_id: voice_state.user_id,
                                flags: Some(voice_state.into()),
                            },
                        ) {
                            error!(?error, "Error sending voice state update");
                        }
                    }
                }
            }
            FullEvent::VoiceStateUpdate { new, .. } => {
                let now = Instant::now();
                if let Some(channel_id) = new.channel_id
                    && let Some(guild_id) = new.guild_id
                    && self.subscription_provider.has_subscription(channel_id)
                {
                    if let Err(err) = self.coordinator_handle.track(
                        channel_id,
                        guild_id,
                        room::VoiceStateUpdate {
                            now,
                            user_id: new.user_id,
                            flags: Some(new.into()),
                        },
                    ) {
                        error!("failed to send message to coordinator: {}", err);
                    }
                } else if let Err(err) = self.coordinator_handle.notify(
                    new.channel_id,
                    room::VoiceStateUpdate {
                        now,
                        user_id: new.user_id,
                        flags: new.channel_id.map(|_| new.into()),
                    },
                ) {
                    error!("failed to send message to coordinator: {}", err);
                }
            }

            _ => {}
        }
    }
}
