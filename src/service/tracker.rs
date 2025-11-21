use std::collections::HashMap;
use serenity::all::{ChannelId, MessageId};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Track {
    pub message_id: MessageId,
    pub last_updated_at: Instant,
}

pub struct Tracker {
    tracks: HashMap<ChannelId, Track>
}

impl Tracker {
    pub fn new() -> Self {
        Tracker {tracks: HashMap::new()}
    }

    pub fn add_track(&mut self, channel_id: ChannelId, message_id: MessageId) {
        let track = Track{
            message_id,
            last_updated_at: Instant::now()
        };
        self.tracks.insert(channel_id, track);
    }

    pub fn update_track(&mut self, channel_id: ChannelId) {
        if let Some(track) = self.tracks.get_mut(&channel_id) {
            track.last_updated_at = Instant::now();
        }
    }

    pub fn get_track(&self, channel_id: &ChannelId) -> Option<&Track> {
        self.tracks.get(channel_id)
    }

    pub fn remove(&mut self, channel_id: ChannelId) {
        self.tracks.remove(&channel_id);
    }
}