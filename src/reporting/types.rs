use crate::room::{
    AudioActivity, Interval, Moment, ParticipantLease, RoomLease, UserIdentity,
};
use serenity::all::{
    ChannelId, CreateAttachment, CreateComponent, CreateMessage, EditAttachments, EditMessage,
    GuildId, Http, MessageFlags, MessageId,
};
use std::borrow::Cow;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub start: Moment,
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub participants: Vec<ParticipantSnapshot>,
}

impl RoomSnapshot {
    pub fn from_lease(room: RoomLease) -> Self {
        RoomSnapshot {
            start: room.start,
            guild_id: room.guild_id,
            channel_id: room.channel_id,
            participants: room
                .participants
                .into_iter()
                .map(ParticipantSnapshot::from_lease)
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistorySnapshot {
    pub audio: Vec<AudioActivity>,
    pub screen_sharing: Vec<Interval>,
}

#[derive(Debug, Clone)]
pub struct ParticipantSnapshot {
    pub identity: UserIdentity,
    pub history: HistorySnapshot,
}

impl ParticipantSnapshot {
    fn from_lease(value: ParticipantLease) -> Self {
        Self {
            identity: value.identity,
            history: HistorySnapshot {
                audio: value.history.audio.to_vec(),
                screen_sharing: value.history.screen_sharing.to_vec(),
            },
        }
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        self.history
            .audio
            .iter()
            .map(|activity| activity.interval.calculate_duration(now))
            .sum()
    }
}

pub struct ReportAnchor {
    channel_id: ChannelId,
    message_id: Option<MessageId>,
}

impl ReportAnchor {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            message_id: None,
        }
    }

    pub async fn sync<'a>(
        &mut self,
        http: &Http,
        component: impl Into<Cow<'a, [CreateComponent<'a>]>>,
        attachment: CreateAttachment<'a>,
    ) -> Result<(), serenity::Error> {
        if let Some(message_id) = self.message_id {
            self.channel_id
                .widen()
                .edit_message(
                    http,
                    message_id,
                    EditMessage::new()
                        .flags(MessageFlags::IS_COMPONENTS_V2)
                        .components(component)
                        .attachments(EditAttachments::new().add(attachment)),
                )
                .await?;

            Ok(())
        } else {
            let message = self
                .channel_id
                .widen()
                .send_message(
                    http,
                    CreateMessage::new()
                        .flags(MessageFlags::IS_COMPONENTS_V2)
                        .components(component)
                        .add_file(attachment),
                )
                .await?;

            self.message_id = Some(message.id);
            Ok(())
        }
    }
}
