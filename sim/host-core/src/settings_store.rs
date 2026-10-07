//! The host's persisted-settings file — the device's settings RRAM stand-in.

use std::path::PathBuf;

use obc_app::Settings;
use obc_ports::SettingsStore;

pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    /// Point the store at `path` (created lazily on the first save).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path: PathBuf = path.into();
        FileSettingsStore { path }
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

    fn save(&mut self, s: &Settings) -> Result<(), obc_ports::SettingsSaveError> {
        let bytes = obc_app::settings::encode(s);
        std::fs::write(&self.path, bytes).map_err(|e| {
            eprintln!("settings: cannot write {}: {e}", self.path.display());
            obc_ports::SettingsSaveError::Backend
        })
    }
}
