use crate::graphics::timeline::layout::LayoutConfig;
use serenity::all::ChannelId;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Subscription {
    pub report_channel: ChannelId,
    pub layout_config: LayoutConfig,
}

pub trait SubscriptionProvider: Send + Sync {
    fn has_subscription(&self, channel: ChannelId) -> bool {
        self.find_subscription(channel).is_some()
    }

    fn find_subscription(&self, voice_channel: ChannelId) -> Option<Subscription>;
}

pub struct StaticSubscriptionProvider {
    subscriptions: HashMap<ChannelId, Subscription>,
}

impl StaticSubscriptionProvider {
    pub fn new(subscriptions: HashMap<ChannelId, Subscription>) -> Self {
        Self { subscriptions }
    }
}

impl SubscriptionProvider for StaticSubscriptionProvider {
    fn find_subscription(&self, voice_channel: ChannelId) -> Option<Subscription> {
        self.subscriptions.get(&voice_channel).cloned()
    }
}
