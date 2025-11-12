use ringring_rs::model::RoomManager;
use ringring_rs::service::report::ReportService;
use serenity::all::{ChannelId, GuildId, Timestamp, VoiceState};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use std::env;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio::time::{self, Duration};
use tracing::{debug, error};

const CLEANUP_INTERVAL_SECS: u64 = 30;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Login with a bot token from the environment
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let report_channel_id = {
        let string_id = env::var("REPORT_CHANNEL_ID").expect("Expected a report channel id in the environment");
        let id = string_id.parse::<u64>().unwrap();
        ChannelId::new(id)
    };

    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;

    // Create a new instance of the Client, logging in as a bot.
    let room_manager = Arc::new(RoomManager::new(16));
    let handler = Handler {
        room_manager: room_manager.clone(),
    };
    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .expect("Err creating client");

    let manager = room_manager.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));

        interval.tick().await;

        loop {
            interval.tick().await;

            let now = Instant::now();
            if let Err(e) = manager.cleanup(now).await {
                error!("Error during room cleanup: {:?}", e);
            }
        }
    });

    let manager = room_manager.clone();
    let http = client.http.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_mins(1));

        let reporter = ReportService::new(reqwest::Client::new(), report_channel_id);
        interval.tick().await;

        loop {
            interval.tick().await;

            for room in manager.get_all_rooms().await {
                let http = http.clone();
                let room = room.lock().await;
                let now = Instant::now();
                match reporter.send_room_report(&http, now, &room).await{
                    Ok(_) => {},
                    Err(e) => {
                        error!("Error sending room report: {:?}", e);
                    }
                }
            }
        }
    });

    // Start listening for events by starting a single shard
    if let Err(why) = client.start().await {
        println!("Client error: {why:?}");
    }
}

fn format_voice_state_nicely(voice_state: &VoiceState) -> String {
    format!(
        "VoiceState {{ channel_id: {:?}, guild_id: {:?}, user_id: {:?} }}",
        voice_state.channel_id, voice_state.guild_id, voice_state.user_id
    )
}

struct Handler {
    room_manager: Arc<RoomManager>,
}

#[async_trait]
impl EventHandler for Handler {
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

    async fn voice_state_update(&self, _ctx: Context, old: Option<VoiceState>, new: VoiceState) {
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
            let flags = (&new).into();
            let member = match new.member {
                Some(member) => member,
                None => {
                    error!("CRITICAL: newly connected Voice State is missing member.");
                    return;
                }
            };
            let name = member.display_name().into();
            let channel_id = match new.channel_id {
                Some(channel_id) => channel_id,
                None => {
                    error!("CRITICAL: newly connected Voice State is missing Channel ID.");
                    return;
                }
            };
            let guild_id = match new.guild_id {
                Some(guild_id) => guild_id,
                None => {
                    error!("CRITICAL: newly connected Voice State is missing Guild ID.");
                    return;
                }
            };
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
                .await{
                Ok(_) => {},
                Err(e) => {
                    error!("Error handling connect event on manager: {:?}", e);
                }
            }
            return;
        }

        // if just disconnected
        if new.channel_id.is_none() {
            let old = match old {
                Some(old) => old,
                None => {
                    error!("CRITICAL: Voice State Update is missing both old and new voice channel");
                    return;
                }
            };
            match manager
                .handle_disconnect_event(now, old.channel_id.unwrap(), new.user_id)
                .await {
                Ok(_) => {},
                Err(e) => {
                    error!("Error handling disconnect event on manager: {:?}", e);
                }
            }
            return;
        }

        // switch channel
        let old = old.unwrap();

        match manager
            .handle_disconnect_event(now, old.channel_id.unwrap(), new.user_id)
            .await {
            Ok(_) => {},
            Err(e) => {
                error!("Error handling disconnect event on manager: {:?}", e);
            }
        }
        let flags = (&new).into();
        let member = match new.member {
            Some(member) => member,
            None => {
                error!("CRITICAL: Voice State is missing member.");
                return;
            }
        };
        let name = member.display_name().into();
        match manager
            .handle_connect_event(
                now,
                timestamp,
                new.channel_id.unwrap(),
                new.guild_id.unwrap(),
                new.user_id,
                name,
                member.face(),
                flags,
            )
            .await {
            Ok(_) => {},
            Err(e) => {
                error!("Error handling connect event on manager: {:?}", e);
            }
        }
        return;
    }
}
