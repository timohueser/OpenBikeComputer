//! Host-side persistent settings — the simulator's stand-in for the device's RRAM.
//!
//! Implements [`obc_ports::SettingsStore`] over a single file holding the shared
//! [`obc_app::settings`] blob — the *same* versioned, CRC'd byte layout the firmware writes to
//! RRAM, so the codec is exercised identically on both sides.
//!
//! The weather alert-mark record (#1542) is the board's second RRAM resident, so it is a second
//! file here, beside the first: two records, two lifetimes, one store object that knows both paths
//! — which is what lets the one-time v16 carry-across read the preferences blob it just wrote off.

use std::path::PathBuf;

use obc_app::weather_alerts::AlertMarks;
use obc_app::{MarksProvenance, Settings};
use obc_ports::SettingsStore;

/// A file-backed settings store, and beside it the alert-mark record's own file.
pub struct FileSettingsStore {
    path: PathBuf,
    marks_path: PathBuf,
}

impl FileSettingsStore {
    /// Point the store at `path` (created lazily on the first save). The marks record takes the
    /// sibling `.marks` file.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path: PathBuf = path.into();
        let marks_path = path.with_extension("marks");
        FileSettingsStore { path, marks_path }
    }

    /// Load the weather alert-mark anchors and say **where they came from** — the board's
    /// [`load_alert_marks`] contract, file-for-line. The record first; failing that, the frozen v16
    /// span of whatever the settings file holds, so an update never costs the rider their anchors.
    ///
    /// [`load_alert_marks`]: obc_app::settings::legacy_alert_marks
    pub fn load_alert_marks(&mut self) -> (AlertMarks, MarksProvenance) {
        if let Some(marks) =
            std::fs::read(&self.marks_path).ok().and_then(|b| obc_app::weather_alerts::decode_alert_marks(&b))
        {
            return (marks, MarksProvenance::Record);
        }
        if let Some(marks) = std::fs::read(&self.path).ok().and_then(|b| obc_app::settings::legacy_alert_marks(&b)) {
            return (marks, MarksProvenance::LegacyBlob);
        }
        (AlertMarks::default(), MarksProvenance::Record)
    }

    /// Persist the alert-mark record — one truncating 64-byte write, like every other tiny sidecar.
    pub fn save_alert_marks(&mut self, marks: &AlertMarks) -> Result<(), obc_ports::SettingsSaveError> {
        std::fs::write(&self.marks_path, obc_app::weather_alerts::encode_alert_marks(marks)).map_err(|e| {
            eprintln!("settings: cannot write {}: {e}", self.marks_path.display());
            obc_ports::SettingsSaveError::Backend
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use obc_app::weather_alerts::AlertMark;

    /// A private directory for one test's two store files.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obc-sim-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("settings.bin")
    }

    /// The marks record round-trips through its own file, and a **first run** — no record and no
    /// settings file at all — reads as no anchors owing no write, not as a torn read.
    #[test]
    fn the_marks_record_round_trips_through_its_own_file() {
        let path = scratch("round-trip");
        let mut store = FileSettingsStore::open(&path);

        let (marks, provenance) = store.load_alert_marks();
        assert_eq!(marks, AlertMarks::default(), "a first run holds no anchors");
        assert_eq!(provenance, MarksProvenance::Record, "…and owes no write");

        let stored: AlertMarks = [
            Some(AlertMark { onset: 1_800_000_900, pos: Some((47_123_456, 8_654_321)), severity: 11 }),
            None,
            Some(AlertMark { onset: -1, pos: None, severity: 3 }),
        ];
        store.save_alert_marks(&stored).expect("the record is written");
        assert_eq!(store.load_alert_marks(), (stored, MarksProvenance::Record), "and read back verbatim");

        // The two records are two files: writing the preferences blob leaves the anchors alone.
        store.save(&Settings::default()).expect("the settings file is written");
        assert_eq!(store.load_alert_marks().0, stored, "a preferences write cannot touch the anchors");
    }
}
