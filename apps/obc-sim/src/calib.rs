//! 1:1 physical-size calibration for the device window.
//!
//! "How big is one millimetre on this screen" is a monitor property the host can't know,
//! so we measure it **once** (ruler on an on-screen reference bar) and persist the
//! resulting *points-per-millimetre*. With that plus the panel's known physical size
//! ([`PANEL_W_MM`]/[`PANEL_H_MM`]), the GUI renders the framebuffer at true size.
//!
//! Everything stays in egui **points** (not physical pixels), so the calibration folds in
//! the OS display-scaling (`pixels_per_point`) automatically — we never query DPI.
//! (Re-calibrate on a different monitor.)

use std::path::PathBuf;

/// The reflective panel's active-area dimensions, in millimetres. Derived from a
/// **2.13″ diagonal** at 240×320 square pixels (a 3:4:5 triangle → width = 0.6·diag,
/// height = 0.8·diag). This is the one number that can't be measured on the host —
/// correct it here if the datasheet's active area differs.
pub const PANEL_W_MM: f32 = 32.46;
pub const PANEL_H_MM: f32 = 43.28;

/// Width (egui points) of the calibration reference bar. The actual drawn width is
/// clamped to the window, and points-per-mm is computed from whatever was drawn, so
/// this is only a target.
pub const REF_BAR_POINTS: f32 = 500.0;

/// Config file holding the one calibrated number (points-per-mm), so 1:1 survives
/// restarts: `$XDG_CONFIG_HOME/obc-sim/calibration` (else `$HOME/.config/...`).
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config_path_under(base))
}

/// The same layout under an explicit config base. Split out so the tests can point at a scratch
/// directory instead of writing `XDG_CONFIG_HOME` — a process-global mutation, and `cargo test`
/// runs the suite on several threads.
fn config_path_under(base: PathBuf) -> PathBuf {
    base.join("obc-sim").join("calibration")
}

/// Load the saved points-per-mm, or `None` if never calibrated / unreadable / invalid.
pub fn load() -> Option<f32> {
    load_from(config_path()?)
}

fn load_from(path: PathBuf) -> Option<f32> {
    let s = std::fs::read_to_string(path).ok()?;
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Persist points-per-mm. Returns a human-readable error (for the panel to show) on
/// failure; best-effort, never panics.
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

    /// `save`/`load` round-trips, and garbage / negative contents read back as `None`. Drives the
    /// path-taking pair against a scratch config base, so nothing here touches the environment the
    /// rest of the suite is reading on other threads.
    #[test]
    fn save_load_roundtrips_and_rejects_junk() {
        let base = obcm_testkit::scratch::scratch_dir("obc-sim-calibtest", "roundtrip");
        let cal = config_path_under(base);

        assert_eq!(load_from(cal.clone()), None, "nothing saved yet");
        save_to(cal.clone(), 4.29).expect("save");
        assert!((load_from(cal.clone()).expect("loads back") - 4.29).abs() < 1e-4);

        // Corrupt / nonsensical contents are ignored rather than trusted.
        let put = |bytes: &str| std::fs::write(&cal, bytes).unwrap();
        put("not a number");
        assert_eq!(load_from(cal.clone()), None);
        put("-3");
        assert_eq!(load_from(cal.clone()), None, "non-positive is invalid");

        // The filter's `> 0.0` (not `>=`) and `is_finite()` boundaries.
        put("0");
        assert_eq!(load_from(cal.clone()), None, "exactly 0 fails `> 0.0` (a zero scale is degenerate)");
        put("0.0");
        assert_eq!(load_from(cal.clone()), None, "0.0 fails `> 0.0`");
        put("nan");
        assert_eq!(load_from(cal.clone()), None, "NaN fails `is_finite()`");
        put("inf");
        assert_eq!(load_from(cal.clone()), None, "infinity fails `is_finite()`");
        // The smallest positive finite value still loads — proves the filter rejects only
        // ≤ 0 and non-finite, not all small numbers.
        put("0.001");
        assert!((load_from(cal).expect("tiny positive is valid") - 0.001).abs() < 1e-6);
    }
}
