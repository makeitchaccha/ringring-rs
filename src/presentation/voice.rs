use crate::room;
use crate::room::CoordinatorHandle;
use serenity::all::{Context, EventHandler, FullEvent};
use serenity::async_trait;
use tokio::time::Instant;
use tracing::{debug, error};

pub struct VoiceHandler {
    coordinator_handle: CoordinatorHandle,
}

impl VoiceHandler {
    pub fn new(coordinator_handle: CoordinatorHandle) -> Self {
        Self { coordinator_handle }
    }
}

#[async_trait]
impl EventHandler for VoiceHandler {
    async fn dispatch(&self, ctx: &Context, event: &FullEvent) {
        match event {
            FullEvent::CacheReady { guilds, .. } => {
                debug!("cache is ready for guilds: {:?}", guilds);
                let now = Instant::now();

                for &guild_id in guilds {
                    let Some(guild_ref) = guild_id.to_guild_cached(&ctx.cache) else {
                        continue;
                    };

                    for voice_state in guild_ref.voice_states.iter() {
                        let Some(channel) = voice_state.channel_id else {
                            continue;
                        };
                        let flags: room::VoiceStateFlags = voice_state.into();
                        if let Err(error) = self.coordinator_handle.track(
                            channel,
                            guild_id,
                            room::VoiceStateUpdate {
                                now,
                                user_id: voice_state.user_id,
                                flags: Some(flags),
                            },
                        ) {
                            error!(?error, "Error sending voice state update");
                        }
                    }
                }
            }
            FullEvent::VoiceStateUpdate { new, .. } => {
                let now = Instant::now();
                let Some(guild_id) = new.guild_id else {
                    error!("Voice state update has no guild id, just ignore");
                    return;
                };

                if let Some(channel_id) = new.channel_id {
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
                    guild_id,
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
