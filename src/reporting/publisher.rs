use crate::graphics::timeline::Renderer;
use crate::graphics::util::Calligraphy;
use crate::infrastructure::AssetProvider;
use crate::reporting::ReportAnchor;
use crate::reporting::reporter::Reporter;
use crate::reporting::subscription::SubscriptionProvider;
use crate::room::CoordinatorEvent;
use serenity::all::Http;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

pub struct Publisher {
    http: Arc<Http>,
    subscription_provider: Arc<dyn SubscriptionProvider>,
    asset_provider: AssetProvider,
    calligraphy: Calligraphy,
    event_rx: mpsc::UnboundedReceiver<CoordinatorEvent>,
}

impl Publisher {
    pub fn new(
        http: Arc<Http>,
        subscription_provider: Arc<dyn SubscriptionProvider>,
        asset_provider: AssetProvider,
        event_rx: mpsc::UnboundedReceiver<CoordinatorEvent>,
    ) -> Self {
        Self {
            http,
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
                    let Some(subscription) =
                        self.subscription_provider.find_subscription(channel_id)
                    else {
                        warn!(
                            "Session pushed event but no valid subscription was found for channel: {}",
                            channel_id
                        );
                        continue;
                    };

                    Reporter::spawn(
                        self.http.clone(),
                        self.asset_provider.clone(),
                        Renderer::new(subscription.layout_config, self.calligraphy.clone()),
                        session_event_rx,
                        ReportAnchor::new(subscription.report_channel),
                    );
                }
            }
        }
    }
}
