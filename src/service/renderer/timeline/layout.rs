use crate::service::renderer::timeline::policy::AspectRatioPolicy;

pub struct LTRB{
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl LTRB{
    pub fn horizontal(&self) -> f32{
        self.left + self.right
    }

    pub fn vertical(&self) -> f32{
        self.top + self.bottom
    }
}

pub struct Layout{
    pub margin: LTRB,
    pub headline_width: f32,
    pub min_timeline_width: f32,
    aspect_ratio_policy: AspectRatioPolicy,
    pub entry_height: f32,
}

impl Layout{
    pub fn height(&self, n_entries: usize) -> f32{
        self.headline_width * n_entries as f32 + self.margin.vertical()
    }

    fn fixed_content_width(&self) -> f32{
        self.headline_width + self.margin.horizontal()
    }

    pub fn timeline_width(&self, n_entries: usize) -> f32{
        self.aspect_ratio_policy.calculate_timeline_width(self.height(n_entries), self.fixed_content_width(), self.min_timeline_width)
    }

    pub fn width(&self, n_entries: usize) -> f32{
        self.timeline_width(n_entries) + self.fixed_content_width()
    }
}
