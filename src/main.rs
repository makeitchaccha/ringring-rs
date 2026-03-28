#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use ringring_rs::infrastructure::AssetProvider;
use ringring_rs::presentation::VoiceHandler;
use ringring_rs::reporting::{Reporter, RoomSnapshot};
use ringring_rs::room::RoomManager;
use serenity::all::ChannelId;
use serenity::prelude::*;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::Instant;
use tokio::time::{self, Duration};
use tracing::error;

const CLEANUP_INTERVAL_SECS: u64 = 30;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Login with a bot token from the environment
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let report_channels = env::var("REPORT_CHANNELS")
        .ok()
        .map(|channels| {
            let channels: Vec<(String, String)> =
                serde_json::from_str(&channels).expect("must be parse as json");
            channels
        })
        .unwrap_or(vec![])
        .iter()
        .map(|p| {
            (
                ChannelId::from_str(p.0.as_str()).expect("must be channel id"),
                ChannelId::from_str(p.1.as_str()).expect("must be channel id"),
            )
        })
        .collect();

    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;

    // Create a new instance of the Client, logging in as a bot.
    let room_manager = Arc::new(RoomManager::new(16));
    let reporter = Arc::new(Reporter::new(
        AssetProvider::new(reqwest::Client::new()),
        report_channels,
    ));
    let handler = VoiceHandler::new(room_manager.clone(), reporter.clone());

    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .expect("Err creating client");

    tokio::spawn({
        let manager = room_manager.clone();
        let http = client.http.clone();
        let reporter = reporter.clone();
        async move {
            let mut interval = time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));

            interval.tick().await;

            loop {
                interval.tick().await;

                let now = Instant::now();
                match manager.cleanup(now).await {
                    Ok(removed) => {
                        for room in removed {
                            let room_guard = room.lock().await;
                            match reporter
                                .send_room_report(
                                    &http,
                                    now,
                                    &RoomSnapshot::from_lease(&room_guard),
                                    false,
                                )
                                .await
                            {
                                Ok(_) => {}
                                Err(err) => {
                                    error!("Failed to send report report: {}", err);
                                    // log error and just ignore
                                    // it may be better if there is retry behavior.
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error during room cleanup: {:?}", e);
                    }
                }
            }
        }
    });

    tokio::spawn({
        let manager = room_manager.clone();
        let reporter = reporter.clone();
        let http = client.http.clone();
        async move {
            let mut interval = time::interval(Duration::from_mins(1));
            interval.tick().await;

            loop {
                interval.tick().await;

                for room in manager.get_all_rooms().await {
                    let http = http.clone();
                    let snapshot = {
                        let room = room.lock().await;
                        RoomSnapshot::from_lease(&room)
                    };
                    let now = Instant::now();
                    match reporter.send_room_report(&http, now, &snapshot, true).await {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Error sending room report: {:?}", e);
                        }
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
