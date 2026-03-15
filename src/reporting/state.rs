use serenity::all::{ChannelId, MessageId};
use std::collections::HashMap;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ReportState {
    pub message_id: MessageId,
    pub last_updated_at: Instant,
}

pub struct ReportStateStore {
    states: HashMap<ChannelId, ReportState>,
}

impl ReportStateStore {
    pub fn new() -> Self {
        ReportStateStore {
            states: HashMap::new(),
        }
    }

    pub fn insert(&mut self, channel_id: ChannelId, message_id: MessageId) {
        let state = ReportState {
            message_id,
            last_updated_at: Instant::now(),
        };
        self.states.insert(channel_id, state);
    }

    pub fn touch(&mut self, channel_id: ChannelId) {
        if let Some(track) = self.states.get_mut(&channel_id) {
            track.last_updated_at = Instant::now();
        }
    }

    pub fn get(&self, channel_id: &ChannelId) -> Option<&ReportState> {
        self.states.get(channel_id)
    }

    pub fn remove(&mut self, channel_id: ChannelId) {
        self.states.remove(&channel_id);
    }
}

impl Default for ReportStateStore {
    fn default() -> Self {
        Self::new()
    }
}
