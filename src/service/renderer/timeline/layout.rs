use tiny_skia::NonZeroRect;
use crate::service::renderer::timeline::policy::AspectRatioPolicy;
use crate::service::renderer::view::Timeline;

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
    pub label_area_height: f32,
    pub avatar_column_width: f32,
    pub min_timeline_width: f32,
    pub aspect_ratio_policy: AspectRatioPolicy,
    pub entry_height: f32,
    pub avatar_size: f32,
}

impl LayoutConfig {
    pub fn calculate(&self, n_entries: usize) -> Layout {
        let total_entry_height = self.entry_height * n_entries as f32;
        let total_height = self.label_area_height + total_entry_height + self.margin.vertical();
        let timeline_width = self.aspect_ratio_policy.calculate_timeline_width(total_height, self.fixed_content_width(), self.min_timeline_width);
        let total_width = timeline_width + self.fixed_content_width();

        Layout {
            total_width,
            total_height,
            avatar_column_width: self.avatar_column_width,
            timeline_width,
            margin: self.margin,
            label_area_height: self.label_area_height,
            entry_height: self.entry_height,
            total_entry_height,
            avatar_size: self.avatar_size,
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
    label_area_height: f32,
    entry_height: f32,
    total_entry_height: f32,
    avatar_column_width: f32,
    timeline_width: f32,
    avatar_size: f32,
}


impl Layout {
    pub fn total_width(&self) -> f32 {
        self.total_width
    }

    pub fn total_height(&self) -> f32 {
        self.total_height
    }

    pub fn avatar_size(&self) -> f32 {
        self.avatar_size
    }

    pub fn full_timeline_bb(&self) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left + self.avatar_column_width,
            self.margin.top + self.label_area_height,
            self.timeline_width,
            self.total_entry_height,
        ).unwrap()
    }

    // returns timeline bounding-box for i-th entry.
    pub fn timeline_bb_for_entry(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left + self.avatar_column_width,
            self.margin.top + self.label_area_height + i as f32 * self.entry_height,
            self.timeline_width,
            self.entry_height,
        ).unwrap()
    }

    // returns headline bounding-box for i-th entry.
    pub fn headline_bb_for_entry(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left,
            self.margin.top + self.label_area_height + i as f32 * self.entry_height,
            self.avatar_column_width,
            self.entry_height,
        ).unwrap()
    }
}
