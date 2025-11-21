use crate::model::{Activity, Participant, Room};
use crate::service::renderer::timeline::{TimelineRenderer, TimelineRendererError};
use crate::service::renderer::view::{FillStyle, VoiceSection, StreamingSection, Timeline, TimelineEntry, Tick};
use serenity::all::{ChannelId, CreateAttachment, CreateMessage, EditAttachments, EditMessage, GuildId, Http, MessageFlags, Timestamp, UserId};
use std::ops::Add;
use std::sync::{Arc};
use std::time::Duration;
use chrono::Local;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::error;
use crate::service::asset::AssetService;
use crate::service::tracker::Tracker;

#[derive(Debug)]
pub enum ReportServiceError{
    GenericError(String),
    RenderingError(TimelineRendererError),
}

impl From<TimelineRendererError> for ReportServiceError {
    fn from(err: TimelineRendererError) -> Self {
        ReportServiceError::RenderingError(err)
    }
}

pub type ReportServiceResult<T> = Result<T, ReportServiceError>;



pub struct ReportService {
    asset_service: AssetService,
    renderer: Arc<TimelineRenderer>,
    report_channel_id: ChannelId,
    tracker: Arc<Mutex<Tracker>>,
}

#[derive(Debug, Clone)]
pub struct RoomDTO {
    pub created_at: Instant,
    pub timestamp: Timestamp,
    pub channel_id: ChannelId,
    pub participants: Vec<Participant>,
}

impl RoomDTO {
    pub fn from_room(room: &Room) -> Self {
        let participants = room.participants().iter().map(|p| {
            p.clone()
        }).collect();

        RoomDTO {
            created_at: room.created_at(),
            timestamp: room.timestamp(),
            channel_id: room.channel_id(),
            participants,
        }
    }
}

impl ReportService {
    pub fn new(asset_service: AssetService, report_channel_id: ChannelId) -> Self {
        Self{
            asset_service,
            renderer: Arc::new(TimelineRenderer::new()),
            report_channel_id,
            tracker: Arc::new(Mutex::new(Tracker::new())),
        }
    }

    async fn create_timeline(&self, now: Instant, now_timestamp: Timestamp, room: &RoomDTO) -> ReportServiceResult<Timeline> {
        let mut entries = Vec::new();

        let terminated_at = calculate_auto_scale(room.created_at, now);

        for participant in &room.participants {
            let visual =
                match self.asset_service.get_members_visual(0.into(), participant.user_id(), participant.face()).await {
                    Ok(visual) => visual,
                    Err(err) => {
                        error!("An error occurred while fetching member visual: {:?}", err);
                        continue;
                    }
                };

            entries.push(TimelineEntry{
                avatar: visual.avatar,
                voice_sections: convert_to_voice_sections(room.created_at, now, terminated_at, participant.history()),
                streaming_sections: convert_to_streaming_sections(room.created_at, now, terminated_at, participant.history()),
                active_color: visual.active_color,
                inactive_color: visual.inactive_color,
                streaming_color: visual.streaming_color,
            });
        }

        let timeline = Timeline{
            created_at: room.created_at,
            terminated_at,
            created_timestamp: room.timestamp.with_timezone(&Local),
            terminated_timestamp: now_timestamp.with_timezone(&Local),
            indicator: Some(now),
            entries,
            tick: choose_suitable_tics(terminated_at - room.created_at),
        };

        Ok(timeline)
    }

    pub async fn send_room_report(&self, http: &Http, now: Instant, now_timestamp: Timestamp, room: &RoomDTO) -> ReportServiceResult<()> {
        let timeline = self.create_timeline(now, now_timestamp, room).await?;

        let renderer = self.renderer.clone();

        let task = tokio::task::spawn_blocking(move || {
            let pixmap = renderer.generate_image(&timeline)?;
            let encoded_image = match pixmap.encode_png() {
                Ok(b) => b,
                Err(err) => {
                    return Err(ReportServiceError::GenericError(err.to_string()))
                }
            };
            Ok(encoded_image)
        });

        let encoded_image = match task.await {
            Ok(image) => image?,
            Err(err) => return Err(ReportServiceError::GenericError(err.to_string()))
        };

        let mut tracker_guard = self.tracker.lock().await;

        match tracker_guard.get_track(&room.channel_id) {
            Some(track) => {
                match self.report_channel_id
                    .edit_message(
                        http,
                        track.message_id,
                        EditMessage::new()
                            .embed(self.renderer.generate_ongoing_embed(
                                now,
                                Timestamp::now(),
                                room,
                            ))
                            .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                            .attachments(EditAttachments::new().add(CreateAttachment::bytes(encoded_image, "thumbnail.png"))),
                    )
                    .await {
                    Ok(_) => Ok(()),
                    Err(err) => Err(ReportServiceError::GenericError(err.to_string())),
                }
            },
            None => {
                match self.report_channel_id
                    .send_message(
                        http,
                        CreateMessage::new()
                            .embed(self.renderer.generate_ongoing_embed(
                                now,
                                Timestamp::now(),
                                room,
                            ))
                            .flags(MessageFlags::SUPPRESS_NOTIFICATIONS)
                            .add_file(CreateAttachment::bytes(encoded_image, "thumbnail.png")),
                    )
                    .await {
                    Ok(message) => {
                        tracker_guard.add_track(room.channel_id, message.id);
                        Ok(())
                    },
                    Err(err) => Err(ReportServiceError::GenericError(err.to_string())),
                }
            }
        }

    }
}

fn convert_to_voice_sections(start: Instant, now: Instant, end: Instant, history: &Vec<Activity>) -> Vec<VoiceSection> {
    let duration_sec = (end - start).as_secs_f32();
    let mut render_sections = Vec::new();

    for i in 0..history.len() {
        let current = &history[i];
        let fill_style = FillStyle::from_flags(current.flags());

        let start_ratio = (current.start() - start).as_secs_f32()/duration_sec;
        let end_ratio = (current.end().unwrap_or(now) - start).as_secs_f32()/duration_sec;

        render_sections.push(VoiceSection {
            start_ratio,
            end_ratio,
            fill_style,
        })
    }

    render_sections
}

fn convert_to_streaming_sections(start: Instant, now: Instant, end: Instant, history: &Vec<Activity>) -> Vec<StreamingSection> {
    let duration_sec = (end - start).as_secs_f32();
    let mut streaming_sections = Vec::new();

    // Always keep streaming start activity.
    let mut streaming_start_activity: Option<&Activity> = None;

    for i in 0..history.len() {
        let current_activity = &history[i];

        // update current streaming start
        match streaming_start_activity {
            Some(streaming_start) => {
                if !current_activity.flags().is_sharing_screen {
                    let start_ratio = (streaming_start.start() - start).as_secs_f32()/duration_sec;
                    let end_ratio = (current_activity.start() - start).as_secs_f32()/duration_sec;

                    streaming_sections.push(StreamingSection{
                        start_ratio,
                        end_ratio,
                    });

                    streaming_start_activity = None;
                }
            },
            None => {
                if current_activity.flags().is_sharing_screen {
                    streaming_start_activity = Some(&history[i]);
                }
            }
        }

        // detect disconnection from voice channel
        if let Some(streaming_start) = &streaming_start_activity {
            let terminated = if i == history.len() - 1 {
                true
            } else {
                !history[i+1].is_following(current_activity)
            };

            if terminated {
                let start_ratio = (streaming_start.start() - start).as_secs_f32()/duration_sec;
                let end_ratio = (current_activity.end().unwrap_or(now) - start).as_secs_f32()/duration_sec;

                streaming_sections.push(StreamingSection{
                    start_ratio,
                    end_ratio,
                });
                streaming_start_activity = None;
            }
        }
    }

    streaming_sections
}

fn calculate_auto_scale(start: Instant, end: Instant) -> Instant {
    const FRAMES: [Duration; 12] = [
        Duration::from_mins(1),
        Duration::from_mins(5),
        Duration::from_mins(10),
        Duration::from_mins(30),
        Duration::from_hours(1),
        Duration::from_hours(2),
        Duration::from_hours(3),
        Duration::from_hours(4),
        Duration::from_hours(6),
        Duration::from_hours(8),
        Duration::from_hours(12),
        Duration::from_hours(24),
    ];

    let duration = end - start;

    for frame in FRAMES {
        if duration < frame {
            return start.add(frame);
        }
    }

    let duration_days = duration.as_secs() / (24 * 60 * 60);

    start.add(Duration::from_hours(24 * (1 + duration_days)))
}

fn choose_suitable_tics(duration: Duration) -> Tick {
    const TICKS: [Tick; 12] = [
        Tick::secs_grain(10),
        Tick::mins_grain(1),
        Tick::mins_grain(2),
        Tick::mins_grain(5),
        Tick::mins_grain(10),
        Tick::mins_grain(15),
        Tick::mins_grain(30),
        Tick::hours_grain(1),
        Tick::hours_grain(2),
        Tick::hours_grain(4),
        Tick::hours_grain(6),
        Tick::hours_grain(12),
    ];

    let duration_secs = duration.as_secs();

    for tick in TICKS {
        if duration_secs / tick.interval.as_secs() < 10 {
            return tick;
        }
    }

    Tick::hours_grain(24)
}