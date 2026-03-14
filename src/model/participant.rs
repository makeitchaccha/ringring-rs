use crate::model::activity::{Activity, ActivityError, ActivityResult, VoiceStateFlags};
use serenity::all::UserId;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct Identification {
    pub user_id: UserId,
    pub name: String,
    pub face: String,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub identification: Identification,
    history: Vec<Activity>,
}

impl Participant {
    pub fn new(identification: Identification) -> Self {
        Participant {
            identification,
            history: Vec::new(),
        }
    }

    pub fn history(&self) -> &Vec<Activity> {
        &self.history
    }

    pub fn is_connected(&self) -> bool {
        self.history.last().is_some_and(|a| a.is_ongoing())
    }

    pub fn connect(&mut self, now: Instant, flags: VoiceStateFlags) -> ActivityResult<()> {
        if self.is_connected() {
            return Err(ActivityError::AlreadyStarted);
        }
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }

    pub fn disconnect(&mut self, now: Instant) -> ActivityResult<()> {
        let last = self
            .history
            .last_mut()
            .ok_or(ActivityError::NoActiveActivity)?;
        last.end_at(now)?;
        Ok(())
    }

    pub fn update(&mut self, now: Instant, flags: VoiceStateFlags) -> Result<(), ActivityError> {
        if !self.is_connected() {
            return Err(ActivityError::NoActiveActivity);
        }

        let last = self
            .history
            .last_mut()
            .expect("is_connected() check failed; this should not happen");
        if last.flags() == flags {
            return Ok(());
        }

        last.end_at(now)?;
        let activity = Activity::start_at(now, flags);
        self.history.push(activity);
        Ok(())
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        let mut duration = Duration::ZERO;
        for activity in &self.history {
            duration += activity.calculate_duration(now)
        }
        duration
    }
}
