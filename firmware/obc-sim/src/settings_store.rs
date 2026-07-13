//! Host-side persistent settings — the simulator's stand-in for the device's RRAM.
//!
//! Implements [`obc_ports::SettingsStore`] over a single file holding the shared
//! [`obc_app::settings`] blob — the *same* versioned, CRC'd byte layout the firmware writes to
//! RRAM, so the codec is exercised identically on both sides.

use std::path::PathBuf;

use obc_app::Settings;
use obc_ports::SettingsStore;

/// A file-backed settings store.
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    /// Point the store at `path` (created lazily on the first save).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        FileSettingsStore { path: path.into() }
    }
}

impl SettingsStore for FileSettingsStore {
    type Value = Settings;

    fn load(&mut self) -> Option<Settings> {
        // A missing file (first run) or an unreadable/short/corrupt blob both yield `None`, so
        // the app starts from `Settings::default` — never a half-parsed value.
        let bytes = std::fs::read(&self.path).ok()?;
        obc_app::settings::decode(&bytes)
    }

    fn save(&mut self, s: &Settings) {
        let bytes = obc_app::settings::encode(s);
        if let Err(e) = std::fs::write(&self.path, bytes) {
            eprintln!("settings: cannot write {}: {e}", self.path.display());
        }
    }
}
