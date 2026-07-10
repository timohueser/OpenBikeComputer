//! Synthetic staged `UPDATE.BIN` for the sim's DFU snapshots (epic #615 S5, #620).
//!
//! The scan/arm runs board-side on the device; the app only ever sees the *result*
//! ([`App::notify_dfu_scan_result`](obc_app::App::notify_dfu_scan_result)). To drive the confirm /
//! error screens headlessly the sim fakes that result — but faithfully: it builds a valid in-memory
//! OBCU container with `obc-dfu`'s encoder and runs the **real** [`armer::scan`] over it (header
//! decode + full CRC-32 + extent resolve), exactly the validation the board's `run_scan` performs,
//! then maps the answer into the app-native [`DfuScanReport`] / [`DfuScanError`].

use obc_app::{DfuScanError, DfuScanReport};
use obc_dfu::armer::{self, ExtentsError, ScanError, StageIo};
use obc_dfu::engine::IoError;
use obc_dfu::{Extent, ImageHeader, HEADER_LEN, MAX_EXTENTS};

/// The sim's stand-in "running" firmware version — what an install would replace. The `same`
/// flavour stages this exact string so the confirm screen's same-version warning renders.
pub const SIM_INSTALLED_VERSION: &str = "v0.9.0-0-gsim0000";

/// The staged version an install would apply (a newer build than [`SIM_INSTALLED_VERSION`]).
const SIM_STAGED_VERSION: &str = "v1.0.0-2-gnew1234";

/// Which confirm-screen shape a `--dfu-scan` snapshot renders.
#[derive(Debug, Clone, Copy)]
pub enum DfuScanKind {
    /// A newer version with a rollback available — no warnings.
    Normal,
    /// The installed version restaged — the same-version warning.
    Same,
    /// A first install (no rollback snapshot) — the no-undo warning.
    First,
}

impl DfuScanKind {
    /// Parse the CLI token.
    pub fn parse(s: &str) -> Result<DfuScanKind, String> {
        match s {
            "normal" => Ok(DfuScanKind::Normal),
            "same" => Ok(DfuScanKind::Same),
            "first" => Ok(DfuScanKind::First),
            other => Err(format!("--dfu-scan needs normal|same|first, got `{other}`")),
        }
    }

    /// The scan report this flavour drives, built from a real scan over a synthetic blob.
    pub fn report(self) -> Result<DfuScanReport, DfuScanError> {
        match self {
            DfuScanKind::Normal => sim_scan_report(SIM_STAGED_VERSION, false),
            DfuScanKind::Same => sim_scan_report(SIM_INSTALLED_VERSION, false),
            DfuScanKind::First => sim_scan_report(SIM_STAGED_VERSION, true),
        }
    }
}

/// An in-memory OBCU container (64-byte header + a small body) backing a real `obc-dfu` scan.
struct SliceStage {
    bytes: Vec<u8>,
}

impl SliceStage {
    /// Encode a valid container tagged `version` over a small dummy body.
    fn build(version: &str) -> Self {
        let body = vec![0xA5u8; 4096];
        let header = ImageHeader::new(&body, version);
        let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
        bytes.extend_from_slice(&header.encode());
        bytes.extend_from_slice(&body);
        SliceStage { bytes }
    }
}

impl StageIo for SliceStage {
    fn stage_len(&mut self) -> Option<u32> {
        Some(self.bytes.len() as u32)
    }

    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let o = offset as usize;
        self.bytes.get(o..o + buf.len()).map(|s| buf.copy_from_slice(s)).ok_or(IoError)
    }

    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        // One contiguous run over the whole synthetic file — a freshly-copied file's FAT shape.
        out[0] = Extent { start_block: 0, blocks: (self.bytes.len() as u32).div_ceil(512) };
        Ok(1)
    }
}

/// Build a scan report for a snapshot by running the real `obc-dfu` scan over a synthetic OBCU
/// blob tagged `staged_version` (== [`SIM_INSTALLED_VERSION`] flags the same-version warning);
/// `first_install` drives the no-undo warning.
pub fn sim_scan_report(staged_version: &str, first_install: bool) -> Result<DfuScanReport, DfuScanError> {
    let mut stage = SliceStage::build(staged_version);
    let mut chunk = [0u8; 512];
    let staged = armer::scan(&mut stage, &mut chunk).map_err(map_scan_error)?;
    Ok(DfuScanReport::new(SIM_INSTALLED_VERSION, staged.header.fw_version_str(), first_install))
}

/// The board's `obc_dfu::ScanError` → app `DfuScanError` fold, mirrored here for the sim.
fn map_scan_error(e: ScanError) -> DfuScanError {
    match e {
        ScanError::Missing => DfuScanError::NotFound,
        ScanError::Io => DfuScanError::Unreadable,
        ScanError::BadHeader | ScanError::BadCrc | ScanError::Truncated => DfuScanError::Damaged,
        ScanError::Oversize => DfuScanError::TooLarge,
        ScanError::TooFragmented { .. } => DfuScanError::TooFragmented,
    }
}
