use std::time::SystemTime;
use serenity::all::VoiceState;

#[derive(Debug)]
pub enum ActivityError {
    AlreadyStarted,
    AlreadyEnded,
    NoActiveActivity,
}

pub type ActivityResult<T> = Result<T, ActivityError>;

pub struct Activity {
    start: SystemTime,
    end: Option<SystemTime>,
    flags: VoiceStateFlags
}

impl Activity {
    pub fn start_at(start: SystemTime, flags: VoiceStateFlags) -> Self {
        Activity{
            start,
            end: None,
            flags,
        }
    }

    pub fn end_at(&mut self, now: SystemTime) -> ActivityResult<()> {
        match self.end {
            Some(end) => Err(ActivityError::AlreadyEnded),
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

    pub fn flags(&self) -> VoiceStateFlags {
        self.flags
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