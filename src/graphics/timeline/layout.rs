use crate::graphics::timeline::policy::AspectRatioPolicy;
use tiny_skia::NonZeroRect;

#[derive(Copy, Clone)]
pub struct Margin {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Margin {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Copy, Clone)]
pub struct LayoutConfig {
    pub margin: Margin,
    pub label_area_height: f32,
    pub avatar_column_width: f32,
    pub min_timeline_width: f32,
    pub aspect_ratio_policy: AspectRatioPolicy,
    pub entry_height: f32,
    pub avatar_size: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            margin: Margin {
                left: 10.0,
                top: 10.0,
                right: 20.0,
                bottom: 10.0,
            },
            label_area_height: 40.0,
            avatar_column_width: 100.0,
            min_timeline_width: 900.0,
            entry_height: 70.0,
            avatar_size: 64.0,
            aspect_ratio_policy: AspectRatioPolicy::min_aspect_ratio_12x7(),
        }
    }
}

impl LayoutConfig {
    /// Computes the layout for a timeline with `n_entries` participants.
    ///
    /// The total image height grows with the number of entries.  The width is
    /// derived from the aspect-ratio policy but clamped to `min_timeline_width`,
    /// so for few participants the image is wider than the target ratio (only
    /// the height grows).  For many participants the image approaches the
    /// target ratio exactly.
    pub fn calculate(&self, n_entries: usize) -> Layout {
        // Guard against zero entries to avoid zero-height bounding boxes.
        let n_entries = n_entries.max(1);
        let total_entry_height = self.entry_height * n_entries as f32;
        let total_height = self.label_area_height + total_entry_height + self.margin.vertical();
        let timeline_width = self.aspect_ratio_policy.calculate_timeline_width(
            total_height,
            self.fixed_content_width(),
            self.min_timeline_width,
        );
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

    fn fixed_content_width(&self) -> f32 {
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
        )
        .unwrap()
    }

    // returns timeline bounding-box for i-th entry.
    pub fn timeline_bb_for_entry(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left + self.avatar_column_width,
            self.margin.top + self.label_area_height + i as f32 * self.entry_height,
            self.timeline_width,
            self.entry_height,
        )
        .unwrap()
    }

    // returns headline bounding-box for i-th entry.
    pub fn headline_bb_for_entry(&self, i: usize) -> NonZeroRect {
        NonZeroRect::from_xywh(
            self.margin.left,
            self.margin.top + self.label_area_height + i as f32 * self.entry_height,
            self.avatar_column_width,
            self.entry_height,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn margin_horizontal_vertical() {
        let m = Margin {
            left: 1.0,
            top: 2.0,
            right: 3.0,
            bottom: 4.0,
        };
        assert_eq!(m.horizontal(), 4.0);
        assert_eq!(m.vertical(), 6.0);
    }

    #[test]
    fn calculate_n_entries_zero_guard() {
        let cfg = LayoutConfig::default();
        let l0 = cfg.calculate(0);
        let l1 = cfg.calculate(1);
        assert_eq!(l0.total_height(), l1.total_height());
        assert_eq!(l0.total_width(), l1.total_width());
    }

    #[test]
    fn calculate_grows_with_entries() {
        let cfg = LayoutConfig::default();
        let l1 = cfg.calculate(1);
        let l3 = cfg.calculate(3);
        assert!(l3.total_height() > l1.total_height());
    }

    #[test]
    fn calculate_basic_values() {
        let cfg = LayoutConfig::default();
        let l = cfg.calculate(2);
        // total_height = label_area_height + entry_height*2 + margin.vertical()
        // margin vertical default 10+10=20
        let expected_h = 40.0 + 70.0 * 2.0 + 20.0;
        assert_eq!(l.total_height(), expected_h);
        assert!(l.total_width() > 0.0);
    }
}
