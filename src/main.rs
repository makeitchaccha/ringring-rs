use ringring_rs::model::RoomManager;
use ringring_rs::service::renderer::timeline::TimelineRenderer;
use serenity::all::{CreateMessage, GuildId, Timestamp, VoiceState};
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

        interval.tick().await;

        let timeline_renderer = TimelineRenderer::new();
        loop {
            interval.tick().await;
            for room in manager.get_all_rooms().await {
                let http = http.clone();
                let room = room.lock().await;
                room.channel_id()
                    .send_message(
                        http,
                        CreateMessage::new().embed(timeline_renderer.generate_ongoing_embed(
                            Instant::now(),
                            Timestamp::now(),
                            &room,
                        )),
                    )
                    .await
                    .unwrap();
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
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!ping" {
            if let Err(why) = msg.channel_id.say(&ctx.http, "Pong!").await {
                println!("Error sending message: {why:?}");
            }
        }
    }

    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        debug!("cache is ready for guilds: {:?}", guilds);

        let manager = self.room_manager.clone();

        let now = Instant::now();
        let timestamp = Timestamp::now();

        let mut tasks = JoinSet::new();

        for guild_id in guilds {
            let guild = ctx.cache.guild(guild_id).unwrap();
            for (user_id, voice_state) in guild.voice_states.iter() {
                let flags = voice_state.into();
                let channel_id = voice_state.channel_id.unwrap();
                let name = guild.members.get(user_id).unwrap().display_name().into();
                let user_id = user_id.into();

                let manager_for_task = manager.clone();

                let connect_task = async move {
                    manager_for_task
                        .handle_connect_event(
                            now, timestamp, channel_id, guild_id, user_id, name, flags,
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
            let flags = (&new).into();
            let name = new.member.unwrap().display_name().into();
            manager
                .handle_connect_event(
                    now,
                    timestamp,
                    new.channel_id.unwrap(),
                    new.guild_id.unwrap(),
                    new.user_id,
                    name,
                    flags,
                )
                .await
                .unwrap();
            return;
        }

        // if just disconnected
        if new.channel_id.is_none() {
            let old = old.unwrap();
            manager
                .handle_disconnect_event(now, old.channel_id.unwrap(), new.user_id)
                .await
                .unwrap();
            return;
        }

        // switch channel
        let old = old.unwrap();
        manager
            .handle_disconnect_event(now, old.channel_id.unwrap(), new.user_id)
            .await
            .unwrap();
        let flags = (&new).into();
        let name = new.member.unwrap().display_name().into();
        manager
            .handle_connect_event(
                now,
                timestamp,
                new.channel_id.unwrap(),
                new.guild_id.unwrap(),
                new.user_id,
                name,
                flags,
            )
            .await
            .unwrap();
        return;
    }
}
