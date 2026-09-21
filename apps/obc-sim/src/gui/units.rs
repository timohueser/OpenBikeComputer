//! Unit conversions and value formatting for the control panel: the zoom and meters-per-pixel
//! relation, the zoom bounds, and the distance and clock strings.

/// Loosely clamp zoom, in pixels per microdegree of latitude, so scroll cannot drive it to zero or
/// infinity and produce a degenerate projection.
pub(super) const MIN_ZOOM: f32 = 1e-6;
pub(super) const MAX_ZOOM: f32 = 1e4;

/// Practical bounds for the zoom slider, in meters per pixel. The mouse can still scroll past
/// them, because the slider writes back only when dragged, so they do not cap the camera.
pub(super) const MPP_MIN: f32 = 0.02;
pub(super) const MPP_MAX: f32 = 20_000.0;

/// Zoom as meters per pixel, re-exported from the renderer's [`obc_render::mpp_for_zoom`], so the
/// panel reads ground scale from the metric the map is drawn with.
pub(super) fn zoom_to_mpp(zoom: f32) -> f32 {
    obc_render::mpp_for_zoom(zoom)
}

/// Meters per pixel as zoom: the inverse of [`zoom_to_mpp`].
pub(super) fn mpp_to_zoom(mpp: f32) -> f32 {
    obc_render::zoom_for_mpp(mpp)
}

/// A ground distance in meters as a short human string.
pub(super) fn format_distance(m: f32) -> String {
    if m < 1.0 {
        format!("{m:.2} m")
    } else if m < 1000.0 {
        format!("{m:.0} m")
    } else {
        format!("{:.1} km", m / 1000.0)
    }
}

/// Seconds as a playback clock: `M:SS`, or `H:MM:SS` past an hour.
pub(super) fn format_clock(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_mpp_roundtrips() {
        for &zoom in &[1e-3_f32, 0.123, 1.0, 42.0, 1e3] {
            let back = mpp_to_zoom(zoom_to_mpp(zoom));
            assert!((back - zoom).abs() < zoom * 1e-5, "zoom {zoom} -> {back}");
        }
    }

    #[test]
    fn zoom_to_mpp_matches_viewport() {
        // The panel's conversion must agree with the renderer's metric, or the ground-span
        // readout would lie about what is on screen.
        let vp = obc_render::Viewport::new(240.0, 320.0, 0, 0, 0.5);
        assert!((zoom_to_mpp(0.5) - vp.meters_per_pixel()).abs() < 1e-5);
    }

    #[test]
    fn distance_formatting() {
        assert_eq!(format_distance(0.4), "0.40 m");
        assert_eq!(format_distance(5.0), "5 m");
        assert_eq!(format_distance(240.0), "240 m");
        assert_eq!(format_distance(2500.0), "2.5 km");
    }

    /// The two exact transition values, which the other test skips.
    #[test]
    fn distance_formatting_hits_exact_boundaries() {
        // Just below 1 m is still the two-decimal sub-meter form.
        assert_eq!(format_distance(0.999), "1.00 m");
        // Exactly 1.0 takes the whole-meter form, not the sub-meter one.
        assert_eq!(format_distance(1.0), "1 m");
        // Just below 1 km stays in meters, and exactly 1000 flips to km.
        assert_eq!(format_distance(999.0), "999 m");
        assert_eq!(format_distance(1000.0), "1.0 km"); // `1000.0 < 1000.0` false → km
    }

    /// Below 1.0, including non-positive values, takes the sub-meter branch. There is no separate
    /// clamp, so a negative distance prints with its sign rather than wrapping.
    #[test]
    fn distance_formatting_zero_and_negative() {
        assert_eq!(format_distance(0.0), "0.00 m");
        assert_eq!(format_distance(-5.0), "-5.00 m");
    }

    /// The three behaviours the GPX scrubber depends on: the `M:SS` form below an hour, the switch
    /// to `H:MM:SS` past one, and a partial second truncating rather than rounding.
    #[test]
    fn clock_formats_minutes_seconds_and_hours() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(5.0), "0:05"); // seconds zero-padded to :02
        assert_eq!(format_clock(65.0), "1:05"); // no leading-zero minute
        assert_eq!(format_clock(600.0), "10:00");
        assert_eq!(format_clock(3599.0), "59:59"); // last second before the hour switch
        assert_eq!(format_clock(3600.0), "1:00:00"); // exactly an hour → H:MM:SS
        assert_eq!(format_clock(3661.0), "1:01:01");
        assert_eq!(format_clock(36000.0), "10:00:00");
        assert_eq!(format_clock(65.9), "1:05"); // truncates, not rounds
    }

    /// A scrubber can hand over a slightly negative time, which must clamp to zero and never
    /// produce a huge value from a negative cast.
    #[test]
    fn clock_clamps_negative_to_zero() {
        assert_eq!(format_clock(-1.0), "0:00");
        assert_eq!(format_clock(-3600.0), "0:00", "even a large negative clamps, not wraps");
    }
}
