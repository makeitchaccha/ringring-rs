use crate::room::{Activity, Moment, ParticipantLease, RoomLease, UserIdentity};
use serenity::all::{
    ChannelId, CreateAttachment, CreateEmbed, CreateMessage, EditAttachments, EditMessage, GuildId,
    MessageId,
};
use serenity::http::CacheHttp;
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
pub struct ParticipantSnapshot {
    pub identity: UserIdentity,
    pub history: Vec<Activity>,
}

impl ParticipantSnapshot {
    fn from_lease(value: ParticipantLease) -> Self {
        Self {
            identity: value.identity,
            history: value.history.to_vec(),
        }
    }

    pub fn calculate_duration(&self, now: Instant) -> Duration {
        self.history
            .iter()
            .map(|activity| activity.calculate_duration(now))
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

    pub async fn sync(
        &mut self,
        http: impl CacheHttp,
        embed: CreateEmbed,
        attachment: CreateAttachment,
    ) -> Result<(), serenity::Error> {
        if let Some(message_id) = self.message_id {
            self.channel_id
                .edit_message(
                    http,
                    message_id,
                    EditMessage::new()
                        .embed(embed)
                        .attachments(EditAttachments::new().add(attachment)),
                )
                .await?;

            Ok(())
        } else {
            let message = self
                .channel_id
                .send_message(http, CreateMessage::new().embed(embed).add_file(attachment))
                .await?;

            self.message_id = Some(message.id);
            Ok(())
        }
    }
}
