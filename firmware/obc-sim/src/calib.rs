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

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("obc-sim").join("calibration"))
}

/// Load the saved points-per-mm, or `None` if never calibrated / unreadable / invalid.
#[cfg(not(target_arch = "wasm32"))]
pub fn load() -> Option<f32> {
    let s = std::fs::read_to_string(config_path()?).ok()?;
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Persist points-per-mm. Returns a human-readable error (for the panel to show) on
/// failure; best-effort, never panics.
#[cfg(not(target_arch = "wasm32"))]
pub fn save(points_per_mm: f32) -> Result<(), String> {
    let path = config_path().ok_or("no $HOME / $XDG_CONFIG_HOME for the config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(&path, format!("{points_per_mm}\n")).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Web build: there's no per-monitor config file, and 1:1 physical sizing makes no
/// sense in a browser canvas, so calibration is never loaded and never persisted.
#[cfg(target_arch = "wasm32")]
pub fn load() -> Option<f32> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn save(_points_per_mm: f32) -> Result<(), String> {
    Err("display calibration is not available in the web build".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `save`/`load` round-trips, and garbage / negative contents read back as `None`.
    /// Redirects the config dir via `XDG_CONFIG_HOME` (this is the only test, so nothing races
    /// on the env).
    #[test]
    fn save_load_roundtrips_and_rejects_junk() {
        let dir = std::env::temp_dir().join(format!("obc-sim-calibtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        assert_eq!(load(), None, "nothing saved yet");
        save(4.29).expect("save");
        assert!((load().expect("loads back") - 4.29).abs() < 1e-4);

        // Corrupt / nonsensical contents are ignored rather than trusted.
        std::fs::write(dir.join("obc-sim").join("calibration"), "not a number").unwrap();
        assert_eq!(load(), None);
        std::fs::write(dir.join("obc-sim").join("calibration"), "-3").unwrap();
        assert_eq!(load(), None, "non-positive is invalid");

        // The filter's `> 0.0` (not `>=`) and `is_finite()` boundaries. Extends this test rather
        // than adding a second, since `load` reads the process-global `XDG_CONFIG_HOME` we own.
        let cal = dir.join("obc-sim").join("calibration");
        std::fs::write(&cal, "0").unwrap();
        assert_eq!(load(), None, "exactly 0 fails `> 0.0` (a zero scale is degenerate)");
        std::fs::write(&cal, "0.0").unwrap();
        assert_eq!(load(), None, "0.0 fails `> 0.0`");
        std::fs::write(&cal, "nan").unwrap();
        assert_eq!(load(), None, "NaN fails `is_finite()`");
        std::fs::write(&cal, "inf").unwrap();
        assert_eq!(load(), None, "infinity fails `is_finite()`");
        // The smallest positive finite value still loads — proves the filter rejects only
        // ≤ 0 and non-finite, not all small numbers.
        std::fs::write(&cal, "0.001").unwrap();
        assert!((load().expect("tiny positive is valid") - 0.001).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
