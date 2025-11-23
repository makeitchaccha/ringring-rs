use crate::model::{Room, RoomError, VoiceStateFlags};
use serenity::all::{ChannelId, GuildId, UserId};
use std::collections::HashMap;
use std::sync::Arc;
use serenity::model::Timestamp;
use thiserror::Error;
use tokio::sync::{Mutex};
use tokio::time::Instant;
use tracing::debug;

pub struct RoomManager{
    shards: Vec<Arc<Mutex<HashMap<ChannelId, Arc<Mutex<Room>>>>>>,
    num_shards: usize
}

#[derive(Debug, Error)]
pub enum RoomManagerError{
    #[error(transparent)]
    Room(RoomError)
}

impl From<RoomError> for RoomManagerError {
    fn from(err: RoomError) -> Self {
        RoomManagerError::Room(err)
    }
}

pub type RoomManagerResult<T> = Result<T, RoomManagerError>;

impl RoomManager {
    pub fn new(num_shards: usize) -> Self {
        let shards = std::iter::repeat_with(|| {
            Arc::new(Mutex::new(HashMap::new()))
        }).take(num_shards).collect();

        RoomManager{
            shards,
            num_shards
        }
    }

    pub async fn get_all_rooms(&self) -> Vec<Arc<Mutex<Room>>> {
        let mut all_rooms = Vec::new();

        for shard_mutex in self.shards.iter() {
            let rooms_guard = shard_mutex.lock().await;

            for room_mutex in rooms_guard.values() {
                all_rooms.push(room_mutex.clone());
            }
        }
        all_rooms
    }

    fn calculate_shard_index(channel_id: ChannelId, num_shards: usize) -> usize{
        (channel_id.get() % num_shards as u64) as usize
    }

    fn get_shard(&self, channel_id: ChannelId) -> &Arc<Mutex<HashMap<ChannelId, Arc<Mutex<Room>>>>> {
        self.shards.get(Self::calculate_shard_index(channel_id, self.num_shards)).unwrap()
    }

    pub async fn handle_connect_event(&self, now: Instant, start: Timestamp, channel_id: ChannelId, guild_id: GuildId, user_id: UserId, name: String, face: String, flags: VoiceStateFlags) -> RoomManagerResult<Arc<Mutex<Room>>> {
        debug!("handle connect event");
        let mut rooms_guard = self.get_shard(channel_id).lock().await;
        let room_guard = rooms_guard.entry(channel_id).or_insert_with(|| {
            debug!("no room found, create new room");
            Arc::new(Mutex::new(Room::new(guild_id, channel_id, now, start)))
        });

        let mut room = room_guard.lock().await;
        room.handle_connect(now, user_id, name, face, flags)?;
        Ok(room_guard.clone())
    }

    pub async fn handle_disconnect_event(&self, now: Instant, channel_id: ChannelId, user_id: UserId) -> RoomManagerResult<()> {
        let rooms_guard = self.get_shard(channel_id).lock().await;
        let room_guard = rooms_guard.get(&channel_id).cloned();
        match room_guard {
            None => {
                debug!("no room to disconnect");
                Ok(())
            },
            Some(room) => {
                let mut room = room.lock().await;
                room.handle_disconnect(now, user_id)?;
                Ok(())
            }
        }
    }

    pub async fn handle_update_event(&self, now: Instant, channel_id: ChannelId, user_id: UserId, flags: VoiceStateFlags) -> RoomManagerResult<()> {
        let rooms_guard = self.get_shard(channel_id).lock().await;
        let room_guard = rooms_guard.get(&channel_id).cloned();
        match room_guard {
            None => {
                debug!("no room to update");
                Ok(())
            },
            Some(room) => {
                let mut room = room.lock().await;
                room.handle_update(now, user_id, flags)?;
                Ok(())
            }
        }
    }

    pub async fn cleanup(&self, now: Instant) -> RoomManagerResult<Vec<ChannelId>> {
        let mut before_cleanup = 0;
        let mut after_cleanup = 0;
        let mut removed = Vec::new();
        for rooms in self.shards.iter() {
            let mut rooms = rooms.lock().await;
            before_cleanup += rooms.iter().count();
            rooms.retain(|&id, room| {
                let has_expired = room.try_lock().map_or(false, |room| { room.has_expired(now) });
                if has_expired {
                    removed.push(id);
                }
                !has_expired
            });
            after_cleanup += rooms.iter().count();
        }
        debug!("{}/{} rooms was cleaned up.", before_cleanup - after_cleanup, before_cleanup);
        Ok(removed)
    }
}