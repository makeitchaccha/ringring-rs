#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use ringring_rs::graphics::timeline::layout::LayoutConfig;
use ringring_rs::infrastructure::AssetProvider;
use ringring_rs::presentation::VoiceHandler;
use ringring_rs::reporting::{Publisher, StaticSubscriptionProvider, Subscription};
use ringring_rs::room::Coordinator;
use serenity::all::{ChannelId, GatewayIntents};
use serenity::prelude::*;
use std::collections::HashMap;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Unable to install TLS");
    tracing_subscriber::fmt::init();
    info!("Starting ringring-rs");

    // 1. Load configuration from environment
    let token = Token::from_env("DISCORD_TOKEN").expect("Expected a token in the environment");

    let subscriptions = env::var("REPORT_CHANNELS")
        .ok()
        .map(|channels_json| {
            let pairs: Vec<(String, String)> = serde_json::from_str(&channels_json)
                .expect("REPORT_CHANNELS must be a valid JSON array of pairs");

            pairs
                .into_iter()
                .map(|(voice_id, report_id)| {
                    let voice_channel =
                        ChannelId::from_str(&voice_id).expect("Invalid voice channel ID");
                    let report_channel =
                        ChannelId::from_str(&report_id).expect("Invalid report channel ID");

                    (
                        voice_channel,
                        Subscription {
                            report_channel,
                            layout_config: LayoutConfig::default(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    let subscription_provider = Arc::new(StaticSubscriptionProvider::new(subscriptions));

    // 2. Initialize infrastructure
    let asset_provider = AssetProvider::new(reqwest::Client::new());

    // 3. Spawn Coordinator (The Heart)
    let (coordinator_handle, coordinator_event_rx) = Coordinator::spawn();

    // 4. Set gateway intents
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;

    // 5. Build Serenity Client
    let mut client = Client::builder(token, intents)
        .event_handler(Arc::new(VoiceHandler::new(
            subscription_provider.clone(),
            coordinator_handle,
        )))
        .await
        .expect("Err creating client");

    // 6. Spawn Publisher (The Bridge)
    let publisher = Publisher::new(
        client.http.clone(),
        subscription_provider.clone(),
        asset_provider,
        coordinator_event_rx,
    );
    tokio::spawn(publisher.run());

    // 8. Start the bot
    info!("Bot is ready and listening for events");
    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
}
