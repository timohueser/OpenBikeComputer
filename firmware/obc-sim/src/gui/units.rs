//! Pure unit conversions and value formatting for the control panel: the zoom ↔
//! meters-per-pixel relation, the zoom bounds, and the human-readable distance /
//! clock strings. Split out of the host loop so the numeric policy stands alone
//! and stays testable on its own (see the tests below).

/// Loosely clamp zoom (pixels per microdegree of latitude) so scroll can't drive
/// it to zero or infinity and produce a degenerate projection.
pub(super) const MIN_ZOOM: f32 = 1e-6;
pub(super) const MAX_ZOOM: f32 = 1e4;

/// Practical bounds for the zoom slider, in meters per pixel: roughly a ~5 m to
/// ~4800 m screen span on the 240 px device. The mouse can still scroll past these
/// (the slider only writes back when dragged), so they don't cap the camera.
pub(super) const MPP_MIN: f32 = 0.02;
pub(super) const MPP_MAX: f32 = 20_000.0;

/// Zoom (px per microdegree-lat) → meters per pixel. Thin re-export of the renderer's
/// own [`obc_render::mpp_for_zoom`] so the control panel reads ground scale from the very
/// metric the map is drawn with — no private copy of the constant or the formula to drift.
pub(super) fn zoom_to_mpp(zoom: f32) -> f32 {
    obc_render::mpp_for_zoom(zoom)
}

/// Meters per pixel → zoom (the inverse of [`zoom_to_mpp`]) — the renderer's
/// [`obc_render::zoom_for_mpp`].
pub(super) fn mpp_to_zoom(mpp: f32) -> f32 {
    obc_render::zoom_for_mpp(mpp)
}

/// A ground distance in meters as a short human string ("5 m", "2.5 km").
pub(super) fn format_distance(m: f32) -> String {
    if m < 1.0 {
        format!("{m:.2} m")
    } else if m < 1000.0 {
        format!("{m:.0} m")
    } else {
        format!("{:.1} km", m / 1000.0)
    }
}

/// Seconds as a playback clock: `M:SS`, or `H:MM:SS` past an hour. Used for the
/// GPX scrubber's position/duration readout.
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
        // The panel's conversion must agree with the renderer's own metric, or the
        // ground-span readout would lie about what's on screen.
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
}
