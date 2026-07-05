//! The host→app BLE **event/state seam** (epic #447, P1): the small app-vocabulary snapshot the
//! host feeds in each pass, plus the store-change signal the object store raises on a commit/delete.
//!
//! `obc-app` stays oblivious to the radio: no `obc-ble` (or board) type crosses this boundary. The
//! host — the board's BLE plane, or the simulator's control panel — distils its link into a
//! [`BleStatus`] and pushes it through [`App::set_ble_status`](crate::App::set_ble_status); the
//! object-store commit/delete paths ring [`App::notify_store_changed`](crate::App::notify_store_changed).
//! The app's own consumers (the connected indicator now; the passkey card and live catalog in later
//! PRs) read only these app-side types.

/// The link state the device UI shows — the whole of what `obc-app` knows about the BLE link.
///
/// Distilled by the host from its radio state (the board's `ble::state` snapshot, or the sim's
/// control panel) and fed in each pass via [`App::set_ble_status`](crate::App::set_ble_status).
/// Deliberately tiny: everything the UI needs, nothing about descriptors, MTUs, or peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BleStatus {
    /// A central holds the link. Drives the connected indicator (menu title bar + Home); absent
    /// (or disconnected) hides it.
    pub connected: bool,
    /// The 6-digit LESC passkey to show while pairing, or `None` otherwise. **Plumbed but not yet
    /// consumed** — the passkey card is P2 (#449); until then this rides the seam so the board's
    /// publish path is complete and the sim can inject it.
    pub passkey: Option<u32>,
}

impl BleStatus {
    /// The powered-but-unlinked default: not connected, no passkey. The app's boot value until the
    /// host feeds the first real snapshot.
    pub const DISCONNECTED: BleStatus = BleStatus { connected: false, passkey: None };
}
