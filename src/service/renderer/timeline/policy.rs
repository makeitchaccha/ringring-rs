pub struct AspectRatioPolicy {
    pub target_width_ratio: f32,
    pub target_height_ratio: f32,
}

impl AspectRatioPolicy {
    pub fn discord_thumbnail_4_3() -> AspectRatioPolicy {
        AspectRatioPolicy {
            target_width_ratio: 4.0,
            target_height_ratio: 3.0,
        }
    }

    pub fn calculate_timeline_width(
        &self,
        total_height: f32,
        fixed_components_width: f32,
        min_timeline_width: f32,
    ) -> f32 {
        let desired_width = self.target_width_ratio * total_height / self.target_height_ratio;
        let desired_timeline_width = desired_width - fixed_components_width;

        if desired_timeline_width < min_timeline_width {
            min_timeline_width
        } else {
            desired_timeline_width
        }
    }
}