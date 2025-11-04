use std::time::SystemTime;
use serenity::all::UserId;
use crate::model::activity::{Activity, ActivityError, ActivityResult, VoiceStateFlags};

pub struct Participant{
    user_id: UserId,
    name: String,
    history: Vec<Activity>
}

impl Participant {
    pub fn new(user_id: UserId, name: String) -> Self {
        Participant{
            user_id,
            name,
            history: Vec::new(),
        }
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_connected(&self) -> bool {
        self.history.last().map_or(false, |a| a.is_ongoing())
    }

    pub fn connect(&mut self, now: SystemTime, flags: VoiceStateFlags) -> ActivityResult<()> {
        if self.is_connected() {
            return Err(ActivityError::AlreadyStarted)
        }
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }

    pub fn disconnect(&mut self, now: SystemTime) -> ActivityResult<()> {
        let last = self.history.last_mut().ok_or(ActivityError::NoActiveActivity)?;
        last.end_at(now)?;
        Ok(())
    }

    pub fn update(&mut self, now: SystemTime, flags: VoiceStateFlags) -> Result<(), ActivityError> {
        if !self.is_connected() {
            return Err(ActivityError::NoActiveActivity)
        }

        let last = self.history.last_mut().expect("is_connected() check failed; this should not happen");
        if last.flags() == flags {
            return Ok(())
        }

        last.end_at(now)?;
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }
}
