use crate::graphics::timeline::Renderer;
use crate::graphics::util::Calligraphy;
use crate::infrastructure::AssetProvider;
use crate::reporting::ReportAnchor;
use crate::reporting::reporter::Reporter;
use crate::reporting::subscription::SubscriptionProvider;
use crate::room::CoordinatorEvent;
use serenity::all::{Cache, Http};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::debug;

pub struct Publisher {
    http: Arc<Http>,
    cache: Arc<Cache>,
    subscription_provider: Arc<dyn SubscriptionProvider>,
    asset_provider: AssetProvider,
    calligraphy: Calligraphy,
    event_rx: mpsc::UnboundedReceiver<CoordinatorEvent>,
}

impl Publisher {
    pub fn new(
        http: Arc<Http>,
        cache: Arc<Cache>,
        subscription_provider: Arc<dyn SubscriptionProvider>,
        asset_provider: AssetProvider,
        event_rx: mpsc::UnboundedReceiver<CoordinatorEvent>,
    ) -> Self {
        Self {
            http,
            cache,
            subscription_provider,
            asset_provider,
            calligraphy: Calligraphy::default(),
            event_rx,
        }
    }

    pub async fn run(mut self) {
        while let Some(event) = self.event_rx.recv().await {
            match event {
                CoordinatorEvent::Published {
                    channel_id,
                    session_event_rx,
                } => {
                    let subscriptions = self.subscription_provider.find_subscriptions(channel_id);

                    if subscriptions.is_empty() {
                        debug!(
                            "Session pushed event but no valid subscription was found for channel: {}",
                            channel_id
                        );
                        continue;
                    }

                    for subscription in subscriptions {
                        Reporter::spawn(
                            self.http.clone(),
                            self.cache.clone(),
                            self.asset_provider.clone(),
                            Renderer::new(subscription.layout_config, self.calligraphy.clone()),
                            session_event_rx.resubscribe(),
                            ReportAnchor::new(subscription.report_channel),
                            subscription.timezone,
                        );
                    }
                }
            }
        }
    }
}
