pub mod timeline;
pub mod transformer;
pub mod view;

pub use timeline::{TimelineRenderer, TimelineRendererError};
pub use transformer::transform;
pub use view::{FillStyle, StreamingSection, Tick, Timeline, TimelineEntry, VoiceSection};
pub use timeline::layout::{LayoutConfig, Margin};
pub use timeline::policy::AspectRatioPolicy;
