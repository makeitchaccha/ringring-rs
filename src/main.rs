mod cli;
mod config;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use crate::config::load_config;
use clap::Parser;
use ringring_rs::graphics::timeline::layout::LayoutConfig;
use ringring_rs::infrastructure::AssetProvider;
use ringring_rs::presentation::VoiceHandler;
use ringring_rs::reporting::{Publisher, StaticSubscriptionProvider, Subscription};
use ringring_rs::room::Coordinator;
use serenity::all::GatewayIntents;
use serenity::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::__private18::AsDynError;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Unable to install TLS");
    tracing_subscriber::fmt::init();
    info!("Starting ringring-rs");

    let cli = cli::Cli::parse();

    let config = match load_config(cli.config.as_path()) {
        Ok(config) => config,
        Err(error) => {
            error!(error = %error.as_dyn_error(), "Failed to load config");
            return;
        }
    };

    let subscriptions = config
        .subscriptions
        .iter()
        .map(|entry| {
            (
                entry.voice_channel,
                Subscription {
                    report_channel: entry.report_channel,
                    layout_config: LayoutConfig::default(),
                    timezone: entry.timezone.unwrap_or(config.timezone),
                },
            )
        })
        .fold(HashMap::new(), |mut map, (voice_channel, subscription)| {
            map.entry(voice_channel)
                .or_insert(Vec::new())
                .push(subscription);
            map
        });

    let subscription_provider = Arc::new(StaticSubscriptionProvider::new(subscriptions));

    // 3. Spawn Coordinator (The Heart)
    let (coordinator_handle, coordinator_event_rx) = Coordinator::spawn();

    // 4. Set gateway intents
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;

    // 5. Build Serenity Client
    let mut client = Client::builder(config.discord.token, intents)
        .event_handler(Arc::new(VoiceHandler::new(
            subscription_provider.clone(),
            coordinator_handle,
        )))
        .await
        .expect("Err creating client");

    let asset_provider = AssetProvider::new(reqwest::Client::new(), client.http.clone());

    // 6. Spawn Publisher (The Bridge)
    let publisher = Publisher::new(
        client.http.clone(),
        client.cache.clone(),
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
