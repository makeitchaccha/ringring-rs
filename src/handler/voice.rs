use std::sync::Arc;
use serenity::all::{Context, EventHandler, GuildId, Message, Timestamp, VoiceState};
use serenity::async_trait;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{debug, error};
use crate::model::{Room, RoomManager};
use crate::service::report::{ReportService, RoomDTO};

pub struct VoiceHandler {
    room_manager: Arc<RoomManager>,
    report_service: Arc<ReportService>,
}

impl VoiceHandler {
    pub fn new(room_manager: Arc<RoomManager>, report_service: Arc<ReportService>) -> Self {
        VoiceHandler { room_manager, report_service }
    }
}

#[async_trait]
impl EventHandler for VoiceHandler {
    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        debug!("cache is ready for guilds: {:?}", guilds);

        let manager = self.room_manager.clone();

        let now = Instant::now();
        let timestamp = Timestamp::now();

        let mut tasks = JoinSet::new();

        for guild_id in guilds {
            let guild = match ctx.cache.guild(guild_id) {
                Some(guild) => guild,
                None => {
                    error!("CRITICAL: Guild ID {} reported by cache_ready event is missing from cache", guild_id);
                    continue;
                }
            };
            for (user_id, voice_state) in guild.voice_states.iter() {
                let flags = voice_state.into();
                let channel_id = match voice_state.channel_id {
                    Some(channel_id) => channel_id,
                    None => {
                        debug!("Voice State for User {} reported by cache_ready event is not joining voice channel", voice_state.user_id);
                        continue;
                    }
                };
                let member = match guild.members.get(user_id) {
                    Some(member) => member,
                    None => {
                        error!("CRITICAL: failed to get member for User ID {} on Guild ID {} from cache", user_id, guild_id);
                        continue;
                    }
                };
                let name = member.display_name().into();
                let face = member.face();
                let user_id = user_id.into();

                let manager_for_task = manager.clone();

                let connect_task = async move {
                    manager_for_task
                        .handle_connect_event(
                            now, timestamp, channel_id, guild_id, user_id, name, face, flags,
                        )
                        .await
                };
                tasks.spawn(connect_task);
            }
        }

        while let Some(res) = tasks.join_next().await {
            if let Err(why) = res {
                debug!("error joining voice channel: {why:?}");
            }
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!ping" {
            if let Err(why) = msg.channel_id.say(&ctx.http, "Pong!").await {
                println!("Error sending message: {why:?}");
            }
        }
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        debug!(
            "voice_state_update: {:?} -> {}",
            old.as_ref().map(|x| format_voice_state_nicely(&x)),
            format_voice_state_nicely(&new)
        );
        let manager = self.room_manager.clone();
        let now = Instant::now();
        let timestamp = Timestamp::now();
        // if newly connected
        if old.is_none() {
            match handle_connect_safely(&manager, now, timestamp, new).await {
                Ok(room) => {
                    let room = room.lock().await;
                    if let Err(err) = self.report_service.send_room_report(&ctx.http, now, &RoomDTO::from_room(&room)).await {
                        error!("Error sending room report: {:?}", err);
                    }
                },
                Err(err) => {
                    error!("Error handling connect event on channel: {err}");
                }
            }
            return;
        }

        // if just disconnected
        if new.channel_id.is_none() {
            if let Err(err) = handle_disconnect_safely(&manager, now, old).await{
                error!("Error handling disconnect event on channel: {err}");
            }
            return;
        }

        // switch channel
        if let Err(err) = handle_disconnect_safely(&manager, now, old).await{
            error!("Error handling disconnect event on channel: {err}");
        }
        match handle_connect_safely(&manager, now, timestamp, new).await {
            Ok(room) => {
                let room = room.lock().await;
                if let Err(err) = self.report_service.send_room_report(&ctx.http, now, &RoomDTO::from_room(&room)).await {
                    error!("Error sending room report: {:?}", err);
                }
            },
            Err(err) => {
                error!("Error handling connect event on channel: {err}");
            }
        }
        return;
    }
}

async fn handle_connect_safely(manager: &RoomManager, now: Instant, timestamp: Timestamp, new: VoiceState) -> Result<Arc<Mutex<Room>>, String> {
    let flags = (&new).into();
    let member = match new.member {
        Some(member) => member,
        None => return Err(String::from("Voice State is missing member"))
    };

    let channel_id = match new.channel_id {
        Some(channel_id) => channel_id,
        None => return Err(String::from("Voice State is missing Channel ID"))
    };

    let guild_id = match new.guild_id {
        Some(guild_id) => guild_id,
        None => return Err(String::from("Voice State is missing Guild ID"))
    };
    let name = member.display_name().into();
    match manager
        .handle_connect_event(
            now,
            timestamp,
            channel_id,
            guild_id,
            new.user_id,
            name,
            member.face(),
            flags,
        )
        .await {
        Ok(room) => Ok(room),
        Err(e) => Err(format!("Error handling connect event on channel: {e:?}")),
    }
}

async fn handle_disconnect_safely(manager: &RoomManager, now: Instant, old: Option<VoiceState>) -> Result<(), String>{
    let old = match old {
        Some(old) => old,
        None => {
            return Err(String::from("Voice State Update is missing old voice channel"))
        }
    };

    let channel_id = match old.channel_id {
        Some(channel_id) => channel_id,
        None => {
            return Err(String::from("Voice State Update is missing channel ID"))
        }
    };

    match manager
        .handle_disconnect_event(now, channel_id, old.user_id)
        .await {
        Ok(_) => { Ok(())},
        Err(err) => {
            Err(format!("Error handling disconnect event on manager: {:?}", err))
        }
    }
}

fn format_voice_state_nicely(voice_state: &VoiceState) -> String {
    format!(
        "VoiceState {{ channel_id: {:?}, guild_id: {:?}, user_id: {:?} }}",
        voice_state.channel_id, voice_state.guild_id, voice_state.user_id
    )
}
