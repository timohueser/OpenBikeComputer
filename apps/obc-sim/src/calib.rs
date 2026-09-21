//! 1:1 physical-size calibration for the device window.
//!
//! How big one millimetre is on this screen is a monitor property the host cannot know, so it is
//! measured once with a ruler on an on-screen reference bar and the points-per-millimetre is
//! persisted. With that and the panel's known physical size, the GUI renders the framebuffer at
//! true size.
//!
//! Everything stays in egui points rather than physical pixels, so the calibration folds in the OS
//! display scaling and nothing queries DPI. A different monitor needs a new calibration.

use std::path::PathBuf;

/// The reflective panel's active-area dimensions in millimetres, derived from the diagonal at
/// square pixels. It is the one number that cannot be measured on the host, so correct it here if
/// the datasheet's active area differs.
pub const PANEL_W_MM: f32 = 32.46;
pub const PANEL_H_MM: f32 = 43.28;

/// Width in egui points of the calibration reference bar. The drawn width is clamped to the
/// window and points-per-mm is computed from what was drawn, so this is only a target.
pub const REF_BAR_POINTS: f32 = 500.0;

/// Config file holding the one calibrated number, so 1:1 survives restarts.
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config_path_under(base))
}

/// The same layout under an explicit config base, so the tests can point at a scratch directory
/// instead of writing `XDG_CONFIG_HOME`, which is a process-global mutation.
fn config_path_under(base: PathBuf) -> PathBuf {
    base.join("obc-sim").join("calibration")
}

/// Load the saved points-per-mm, or `None` when it was never calibrated or cannot be read.
pub fn load() -> Option<f32> {
    load_from(config_path()?)
}

fn load_from(path: PathBuf) -> Option<f32> {
    let s = std::fs::read_to_string(path).ok()?;
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Persist points-per-mm. It answers a message the panel can show on failure and never panics.
pub fn save(points_per_mm: f32) -> Result<(), String> {
    save_to(config_path().ok_or("no $HOME / $XDG_CONFIG_HOME for the config dir")?, points_per_mm)
}

fn save_to(path: PathBuf, points_per_mm: f32) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(&path, format!("{points_per_mm}\n")).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// It drives the path-taking pair against a scratch config base, so nothing here touches the
    /// environment the rest of the suite reads on other threads.
    #[test]
    fn save_load_roundtrips_and_rejects_junk() {
        let base = obcm_testkit::scratch::scratch_dir("obc-sim-calibtest", "roundtrip");
        let cal = config_path_under(base);

        assert_eq!(load_from(cal.clone()), None, "nothing saved yet");
        save_to(cal.clone(), 4.29).expect("save");
        assert!((load_from(cal.clone()).expect("loads back") - 4.29).abs() < 1e-4);

        // Corrupt contents are ignored rather than trusted.
        let put = |bytes: &str| std::fs::write(&cal, bytes).unwrap();
        put("not a number");
        assert_eq!(load_from(cal.clone()), None);
        put("-3");
        assert_eq!(load_from(cal.clone()), None, "non-positive is invalid");

        // The filter's positive and finite boundaries.
        put("0");
        assert_eq!(load_from(cal.clone()), None, "exactly 0 fails `> 0.0` (a zero scale is degenerate)");
        put("0.0");
        assert_eq!(load_from(cal.clone()), None, "0.0 fails `> 0.0`");
        put("nan");
        assert_eq!(load_from(cal.clone()), None, "NaN fails `is_finite()`");
        put("inf");
        assert_eq!(load_from(cal.clone()), None, "infinity fails `is_finite()`");
        // The smallest positive finite value still loads, so the filter rejects only
        // non-positive and non-finite numbers.
        put("0.001");
        assert!((load_from(cal).expect("tiny positive is valid") - 0.001).abs() < 1e-6);
    }
}
