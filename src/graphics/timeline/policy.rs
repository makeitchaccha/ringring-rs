/// Controls how the timeline image width is derived from its height.
///
/// The policy computes a **minimum** aspect ratio: the image is never narrower
/// than `min_width_ratio : min_height_ratio`, but may be wider when the
/// timeline width is clamped to `min_timeline_width` (typical for few
/// participants).
#[derive(Copy, Clone)]
pub struct AspectRatioPolicy {
    /// Width component of the minimum ratio.
    pub min_width_ratio: f32,
    /// Height component of the minimum ratio.
    pub min_height_ratio: f32,
}

impl AspectRatioPolicy {
    /// Creates a policy with a minimum aspect ratio of 12:7 (width:height).
    ///
    /// # Behavior
    /// - For few participants, `min_timeline_width` dominates and the image is
    ///   wider than 12:7 (width stays fixed, height grows with participants).
    /// - For many participants (8+), the image approaches exactly 12:7.
    ///
    /// The 12:7 ratio is a **minimum**, not a fixed one.  The timeline width
    /// is derived from the total height, but clamped to `min_timeline_width`
    /// so that small timelines are not too narrow.
    ///
    /// # Why 12:7
    /// 12:7 (≈ 1.71:1) matches Discord's embed preview aspect ratio, so the
    /// image renders edge-to-edge (left to right) in the Discord UI without
    /// cropping or letterboxing.  For few participants the minimum width
    /// dominates and the image is wider than 12:7, but as participants grow
    /// the image approaches exactly 12:7 and fills the preview area.
    pub fn min_aspect_ratio_12x7() -> AspectRatioPolicy {
        AspectRatioPolicy {
            min_width_ratio: 12.0,
            min_height_ratio: 7.0,
        }
    }

    /// Calculates the timeline width from the total image height.
    ///
    /// `desired_width = ratio × total_height`.  The timeline width is
    /// `desired_width - fixed_components_width`, clamped to
    /// `min_timeline_width`.
    ///
    /// Because `total_height` grows with the number of participants, the
    /// desired width also grows — but only when it exceeds
    /// `min_timeline_width`.  For typical participant counts (1-7), the
    /// minimum width dominates and the image grows **vertically only**.
    pub fn calculate_timeline_width(
        &self,
        total_height: f32,
        fixed_components_width: f32,
        min_timeline_width: f32,
    ) -> f32 {
        let desired_width = self.min_width_ratio * total_height / self.min_height_ratio;
        let desired_timeline_width = desired_width - fixed_components_width;

        if desired_timeline_width < min_timeline_width {
            min_timeline_width
        } else {
            desired_timeline_width
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn calculate_timeline_width_clamped_to_min() {
        let p = AspectRatioPolicy {
            min_width_ratio: 12.0,
            min_height_ratio: 7.0,
        };
        let w = p.calculate_timeline_width(100.0, 110.0, 900.0);
        assert_eq!(w, 900.0);
    }

    #[test]
    fn calculate_timeline_width_ratio_path() {
        let p = AspectRatioPolicy {
            min_width_ratio: 12.0,
            min_height_ratio: 7.0,
        };
        let w = p.calculate_timeline_width(1000.0, 110.0, 900.0);
        let expected = 12.0 * 1000.0 / 7.0 - 110.0;
        assert!((w - expected).abs() < 1e-3);
    }

    #[test]
    fn min_aspect_ratio_12x7_constructor() {
        let p = AspectRatioPolicy::min_aspect_ratio_12x7();
        assert_eq!(p.min_width_ratio, 12.0);
        assert_eq!(p.min_height_ratio, 7.0);
    }
}
