use anyhow::Context;
use config::{Config, Environment, File};
use serde::Deserialize;
use serenity::all::ChannelId;
use std::path::Path;

pub fn load_config(path: &Path) -> anyhow::Result<AppConfig> {
    let config = Config::builder()
        .add_source(File::from(path))
        .add_source(Environment::with_prefix("RINGRING"))
        .build()
        .context("failed to build config")?;

    let app_config: AppConfig = config
        .try_deserialize()
        .context("failed to deserialize config")?;

    Ok(app_config)
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub discord_token: String,
    pub subscriptions: Vec<SubscriptionEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionEntry {
    pub voice_channel: ChannelId,
    pub report_channel: ChannelId,
}
