//! 1:1 physical-size calibration for the device window.
//!
//! The host can render the device image at any size, but "how big is one
//! millimetre on this screen" is a property of the monitor it can't know. So we
//! measure it **once** — the user holds a ruler to an on-screen reference bar and
//! types its length — and persist the resulting *points-per-millimetre*. With that
//! plus the panel's known physical size ([`PANEL_W_MM`]/[`PANEL_H_MM`]), the GUI
//! renders the 240×320 framebuffer at the panel's true size.
//!
//! Everything stays in egui **points** (not physical pixels): the reference bar is
//! drawn in points and the device image is sized in points, so the calibration folds
//! in the OS display-scaling (`pixels_per_point`, e.g. 2× on a Retina Mac)
//! automatically — we never query DPI. (Re-calibrate if you move to a different
//! monitor.)

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
    Some(base.join("obc-sim").join("calibration"))
}

/// Load the saved points-per-mm, or `None` if never calibrated / unreadable / invalid.
pub fn load() -> Option<f32> {
    let s = std::fs::read_to_string(config_path()?).ok()?;
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Persist points-per-mm. Returns a human-readable error (for the panel to show) on
/// failure; best-effort, never panics.
pub fn save(points_per_mm: f32) -> Result<(), String> {
    let path = config_path().ok_or("no $HOME / $XDG_CONFIG_HOME for the config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(&path, format!("{points_per_mm}\n"))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `save` then `load` round-trips, and garbage / negative contents read back as
    /// `None`. Redirects the config dir via `XDG_CONFIG_HOME` (edition 2021: `set_var`
    /// is safe; this is the crate's only test, so nothing races on the env).
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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
