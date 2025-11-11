use tiny_skia::NonZeroRect;
use crate::service::renderer::timeline::policy::AspectRatioPolicy;

#[derive(Copy, Clone)]
pub struct Margin {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Margin {
    pub fn horizontal(&self) -> f32{
        self.left + self.right
    }

    pub fn vertical(&self) -> f32{
        self.top + self.bottom
    }
}

pub struct LayoutConfig {
    pub margin: Margin,
    pub avatar_column_width: f32,
    pub min_timeline_width: f32,
    pub aspect_ratio_policy: AspectRatioPolicy,
    pub entry_height: f32,
}

impl LayoutConfig {
    pub fn calculate(&self, n_entries: usize) -> Layout {
        let total_height = self.entry_height * n_entries as f32 + self.margin.vertical();
        let timeline_width = self.aspect_ratio_policy.calculate_timeline_width(total_height, self.fixed_content_width(), self.min_timeline_width);
        let total_width = timeline_width + self.fixed_content_width();

        Layout {
            total_width,
            total_height,
            avatar_column_width: self.avatar_column_width,
            timeline_width,
            margin: self.margin,
            entry_height: self.entry_height,
        }
    }

    fn fixed_content_width(&self) -> f32{
        self.avatar_column_width + self.margin.horizontal()
    }
}

pub struct Layout {
    total_width: f32,
    total_height: f32,

    margin: Margin,
    entry_height: f32,
    avatar_column_width: f32,
    timeline_width: f32,
}


impl Layout {
    pub fn total_width(&self) -> f32 {
        self.total_width
    }

    pub fn total_height(&self) -> f32 {
        self.total_height
    }

    // returns timeline bounding-box for i-th entry.
    pub fn timeline_bb(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left + self.avatar_column_width,
            self.margin.top + i as f32 * self.entry_height,
            self.timeline_width,
            self.entry_height,
        ).unwrap()
    }

    // returns headline bounding-box for i-th entry.
    pub fn headline_bb(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left,
            self.margin.top + i as f32 * self.entry_height,
            self.avatar_column_width,
            self.entry_height,
        ).unwrap()
    }
}
