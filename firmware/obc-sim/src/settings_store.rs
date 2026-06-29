//! Host-side persistent settings — the simulator's stand-in for the device's RRAM.
//!
//! Implements [`obc_app::SettingsStore`] over a single file holding the shared
//! [`obc_app::settings`] blob (the *same* versioned, CRC'd byte layout the firmware writes to
//! RRAM), so the codec is exercised identically on both sides. The app seeds itself from
//! [`load`](obc_app::SettingsStore::load) at boot and the GUI calls
//! [`save`](obc_app::SettingsStore::save) whenever the app reports a settings change — so
//! quitting and relaunching restores units / clock / GPS interval.
//!
//! On the web (wasm) there is no filesystem, so the store is a no-op: settings live for the
//! session only, mirroring the web [`TrackStore`](crate::track::TrackStore).

use std::path::PathBuf;

use obc_app::{Settings, SettingsStore};

/// A file-backed settings store. Native reads/writes the file; the web build keeps no file.
pub struct FileSettingsStore {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    path: PathBuf,
}

impl FileSettingsStore {
    /// Point the store at `path` (created lazily on the first save).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        FileSettingsStore { path: path.into() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SettingsStore for FileSettingsStore {
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

// No filesystem in the browser: the store accepts and discards, so settings are session-only.
#[cfg(target_arch = "wasm32")]
impl SettingsStore for FileSettingsStore {
    fn load(&mut self) -> Option<Settings> {
        None
    }
    fn save(&mut self, _s: &Settings) {}
}
