#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use ringring_rs::handler::voice::VoiceHandler;
use ringring_rs::model::RoomManager;
use ringring_rs::service::asset::AssetService;
use ringring_rs::service::report::{ReportService, RoomDTO};
use serenity::all::{ChannelId, Timestamp};
use serenity::prelude::*;
use std::env;
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

    let report_channel_id = {
        let string_id = env::var("REPORT_CHANNEL_ID").expect("Expected a report channel id in the environment");
        let id = string_id.parse::<u64>().unwrap();
        ChannelId::new(id)
    };

    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;

    // Create a new instance of the Client, logging in as a bot.
    let room_manager = Arc::new(RoomManager::new(16));
    let report_service = Arc::new(ReportService::new(AssetService::new(reqwest::Client::new()), report_channel_id));
    let handler = VoiceHandler::new(room_manager.clone(), report_service.clone());

    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .expect("Err creating client");

    // let manager = room_manager.clone();
    // tokio::spawn(async move {
    //     let mut interval = time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));
    //
    //     interval.tick().await;
    //
    //     loop {
    //         interval.tick().await;
    //
    //         let now = Instant::now();
    //         if let Err(e) = manager.cleanup(now).await {
    //             error!("Error during room cleanup: {:?}", e);
    //         }
    //     }
    // });

    let manager = room_manager.clone();
    let reporter = report_service.clone();
    let http = client.http.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_mins(1));
        interval.tick().await;

        loop {
            interval.tick().await;

            for room in manager.get_all_rooms().await {
                let http = http.clone();
                let room_dto = {
                    let room = room.lock().await;
                    RoomDTO::from_room(&room)
                };
                let now = Instant::now();
                match reporter.send_room_report(&http, now, &room_dto).await{
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
