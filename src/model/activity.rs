use std::time::Duration;
use serenity::all::VoiceState;
use thiserror::Error;
use tokio::time::Instant;

#[derive(Debug, Error)]
pub enum ActivityError {
    #[error("Activity has already started")]
    AlreadyStarted,

    #[error("Activity has already ended")]
    AlreadyEnded,

    #[error("No activity found")]
    NoActiveActivity,
}

pub type ActivityResult<T> = Result<T, ActivityError>;

#[derive(Debug, Clone)]
pub struct Activity {
    start: Instant,
    end: Option<Instant>,
    flags: VoiceStateFlags
}

impl Activity {
    pub fn start_at(start: Instant, flags: VoiceStateFlags) -> Self {
        Activity{
            start,
            end: None,
            flags,
        }
    }

    pub fn end_at(&mut self, now: Instant) -> ActivityResult<()> {
        match self.end {
            Some(_) => Err(ActivityError::AlreadyEnded),
            None => {
                self.end = Some(now);
                Ok(())
            }
        }
    }

    pub fn is_ended(&self) -> bool {
        self.end.is_some()
    }

    pub fn is_ongoing(&self) -> bool {
        self.end.is_none()
    }

    pub fn is_following(&self, prev: &Activity) -> bool {
        prev.end.map_or(false, |end| {end == self.start})
    }

    pub fn start(&self) -> Instant {
        self.start
    }

    pub fn end(&self) -> Option<Instant> {
        self.end
    }
    
    pub fn flags(&self) -> VoiceStateFlags {
        self.flags
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        if let Some(end) = self.end {
            end.duration_since(self.start)
        } else {
            now.duration_since(self.start)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceStateFlags {
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_sharing_screen: bool,
}

impl From<&VoiceState> for VoiceStateFlags {
    fn from(state: &VoiceState) -> Self {
        VoiceStateFlags {
            is_muted: state.mute || state.self_mute,
            is_deafened: state.deaf || state.self_deaf,
            is_sharing_screen: state.self_stream.unwrap_or(false)
        }
    }
}