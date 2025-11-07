use std::env;
use std::sync::Arc;
use std::time::SystemTime;
use serenity::all::{GuildId, VoiceState};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::time::Instant;
use tracing::debug;
use ringring_rs::model::RoomManager;


#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Login with a bot token from the environment
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_VOICE_STATES;

    // Create a new instance of the Client, logging in as a bot.
    let handler = Handler{room_manager: Arc::new(RoomManager::new())};
    let mut client =
        Client::builder(&token, intents).event_handler(handler).await.expect("Err creating client");

    // Start listening for events by starting a single shard
    if let Err(why) = client.start().await {
        println!("Client error: {why:?}");
    }
}

fn format_voice_state_nicely(voice_state: &VoiceState) -> String {
    format!("VoiceState {{ channel_id: {:?}, guild_id: {:?}, user_id: {:?} }}", voice_state.channel_id, voice_state.guild_id, voice_state.user_id)
}

struct Handler{
    room_manager: Arc<RoomManager>
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
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        debug!("voice_state_update: {:?} -> {}", old.as_ref().map(|x| format_voice_state_nicely(&x)), format_voice_state_nicely(&new));
        let manager = self.room_manager.clone();
        let now = Instant::now();
        let timestamp = SystemTime::now();
        // if newly connected
        if old.is_none() {
            let flags = (&new).into();
            let name = new.member.unwrap().display_name().into();
            manager.handle_connect_event(
                now,
                timestamp,
                new.channel_id.unwrap(),
                new.guild_id.unwrap(),
                new.user_id,
                name,
                flags,
            ).await.unwrap();
            return;
        }

        // if just disconnected
        if new.channel_id.is_none() {
            let old = old.unwrap();
            manager.handle_disconnect_event(
                now,
                old.channel_id.unwrap(),
                new.user_id
            ).await.unwrap();
            return;
        }

        // switch channel
        let old = old.unwrap();
        manager.handle_disconnect_event(
            now,
            old.channel_id.unwrap(),
            new.user_id
        ).await.unwrap();
        let flags = (&new).into();
        let name = new.member.unwrap().display_name().into();
        manager.handle_connect_event(
            now,
            timestamp,
            new.channel_id.unwrap(),
            new.guild_id.unwrap(),
            new.user_id,
            name,
            flags,
        ).await.unwrap();
        return;
    }
}

