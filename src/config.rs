use anyhow::Context;
use chrono_tz::Tz;
use config::{Config, Environment, File};
use serde::Deserialize;
use serenity::all::ChannelId;
use serenity::prelude::Token;
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
    pub discord: DiscordConfig,
    pub subscriptions: Vec<SubscriptionEntry>,
    pub timezone: Tz,
    /// Idle timeout in seconds before a session is shut down. Default: 60.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordConfig {
    pub token: Token,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionEntry {
    pub voice_channel: ChannelId,
    pub report_channel: ChannelId,
    pub timezone: Option<Tz>,
    /// Image format for the timeline report. Default: "webp".
    #[serde(default = "default_image_format")]
    pub image_format: String,
}

fn default_image_format() -> String {
    "webp".to_string()
}

fn default_idle_timeout() -> u64 {
    60
}
