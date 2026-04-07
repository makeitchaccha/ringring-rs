use crate::graphics::timeline::layout::LayoutConfig;
use chrono_tz::Tz;
use serenity::all::ChannelId;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Subscription {
    pub report_channel: ChannelId,
    pub layout_config: LayoutConfig,
    pub timezone: Tz,
}

pub trait SubscriptionProvider: Send + Sync {
    fn has_subscription(&self, channel: ChannelId) -> bool {
        !self.find_subscriptions(channel).is_empty()
    }

    fn find_subscriptions(&self, voice_channel: ChannelId) -> Vec<Subscription>;
}

pub struct StaticSubscriptionProvider {
    subscriptions: HashMap<ChannelId, Vec<Subscription>>,
}

impl StaticSubscriptionProvider {
    pub fn new(subscriptions: HashMap<ChannelId, Vec<Subscription>>) -> Self {
        Self { subscriptions }
    }
}

impl SubscriptionProvider for StaticSubscriptionProvider {
    fn find_subscriptions(&self, voice_channel: ChannelId) -> Vec<Subscription> {
        self.subscriptions
            .get(&voice_channel)
            .cloned()
            .unwrap_or(Vec::new())
    }
}
