use crate::model::{Room, RoomError, VoiceStateFlags};
use serenity::all::{ChannelId, GuildId, UserId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

pub struct RoomManager{
    rooms: Arc<Mutex<HashMap<ChannelId, Arc<RwLock<Room>>>>>
}

pub enum RoomManagerError{
    Room(RoomError)
}

impl From<RoomError> for RoomManagerError {
    fn from(err: RoomError) -> Self {
        RoomManagerError::Room(err)
    }
}

pub type RoomManagerResult<T> = Result<T, RoomManagerError>;

impl RoomManager {
    pub fn new() -> Self {
        RoomManager{
            rooms: Arc::new(Mutex::new(HashMap::new()))
        }
    }

    pub async fn handle_connect_event(&mut self, now: Instant, start: SystemTime, channel_id: ChannelId, guild_id: GuildId, user_id: UserId, name: String, flags: VoiceStateFlags) -> RoomManagerResult<()> {
        let room_guard = {
            let mut rooms_guard = self.rooms.lock().await;
            rooms_guard.entry(channel_id).or_insert_with(|| {
                Arc::new(RwLock::new(Room::new(guild_id, channel_id, now, start)))
            }).clone()
        };

        let mut room = room_guard.write().await;
        room.handle_connect(now, user_id, name, flags)?;
        Ok(())
    }

    pub async fn handle_disconnect_event(&mut self, now: Instant, channel_id: ChannelId, user_id: UserId) -> RoomManagerResult<()> {
        let room_guard = {
            let rooms_guard = self.rooms.lock().await;
            rooms_guard.get(&channel_id).expect("must exist").clone()
        };

        let mut room = room_guard.write().await;
        room.handle_disconnect(now, user_id)?;
        Ok(())
    }

    pub async fn handle_update_event(&mut self, now: Instant, channel_id: ChannelId, user_id: UserId, flags: VoiceStateFlags) -> RoomManagerResult<()> {
         let room_guard = {
            let rooms_guard = self.rooms.lock().await;
            rooms_guard.get(&channel_id).expect("must exist").clone()
        };

        let mut room = room_guard.write().await;
        room.handle_update(now, user_id, flags)?;
        Ok(())
    }

    // ⚠️ PERFORMANCE WARNING: POTENTIAL BOTTLENECK
    // Rationale: This function acquires a global lock on the `rooms` Mutex and holds it
    // for the entire duration of the `retain` operation. The `retain` method performs
    // an O(N) scan over all entries in the HashMap.
    //
    // Risk: As the number of total rooms (N) increases, the time this lock is held
    // will grow linearly. This can starve other concurrent tasks (e.g., `handle_connect`,
    // `handle_disconnect`) that are waiting for the same lock.
    //
    // Consequence: This function may become a system-wide bottleneck,
    // significantly reducing the manager's responsiveness and throughput if the
    // number of rooms becomes very large.
    pub async fn cleanup(&mut self, now: Instant) -> RoomManagerResult<()> {
        let mut rooms_guard = self.rooms.lock().await;
        rooms_guard.retain(|_channel_id, room_arc| {
           if Arc::strong_count(room_arc) > 1 {
               // SAFETY GUARD: ZOMBIE ROOM PREVENTION.
               // If strong_count > 1, another task holds a reference (is using the Room).
               // We must retain (fail-safe/safe-side) to prevent race condition.
               return true;
           }

            let room = room_arc.try_read().expect("Room lock poisoned");
            !room.has_expired(now)
        });
        Ok(())
    }
}