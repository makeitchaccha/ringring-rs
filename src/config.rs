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


#[cfg(test)]
mod config_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // Placeholder with the shape Token requires (3 dot-separated parts); not a real secret.
    const TOKEN: &str = "test.test.test";

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ringring-test-{}-{}.toml", std::process::id(), name))
    }

    #[test]
    fn missing_file_errors() {
        let p = std::env::temp_dir().join("ringring-test-nonexistent-xyz.toml");
        let _ = fs::remove_file(&p);
        assert!(load_config(&p).is_err());
    }

    #[test]
    fn load_config_defaults() {
        let p = temp_path("defaults");
        let content = format!(
            r#"
timezone = "UTC"

[discord]
token = "{TOKEN}"

[[subscriptions]]
voice_channel = 1
report_channel = 2

"#
        );
        fs::write(&p, content).unwrap();
        let cfg = load_config(&p).expect("load");
        assert_eq!(cfg.idle_timeout_secs, 60);
        assert_eq!(cfg.subscriptions.len(), 1);
        assert_eq!(cfg.subscriptions[0].image_format, "webp");
        assert!(cfg.subscriptions[0].timezone.is_none());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn load_config_tz_parse() {
        let p = temp_path("tz");
        let content = format!(
            r#"
timezone = "Europe/Berlin"
idle_timeout_secs = 120

[discord]
token = "{TOKEN}"

[[subscriptions]]
voice_channel = 1
report_channel = 2
timezone = "Asia/Tokyo"
image_format = "png"
"#
        );
        fs::write(&p, content).unwrap();
        let cfg = load_config(&p).expect("load");
        assert_eq!(cfg.idle_timeout_secs, 120);
        // Tz equality via string form
        assert_eq!(cfg.timezone.to_string(), "Europe/Berlin");
        assert_eq!(cfg.subscriptions[0].image_format, "png");
        assert_eq!(
            cfg.subscriptions[0].timezone.unwrap().to_string(),
            "Asia/Tokyo"
        );
        let _ = fs::remove_file(&p);
    }
}
