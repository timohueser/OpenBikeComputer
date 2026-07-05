//! The host→app BLE **event/state seam** (epic #447, P1): the small app-vocabulary snapshot the
//! host feeds in each pass, plus the store-change signal the object store raises on a commit/delete.
//!
//! `obc-app` stays oblivious to the radio: no `obc-ble` (or board) type crosses this boundary. The
//! host — the board's BLE plane, or the simulator's control panel — distils its link into a
//! [`BleStatus`] and pushes it through [`App::set_ble_status`](crate::App::set_ble_status); the
//! object-store commit/delete paths ring [`App::notify_store_changed`](crate::App::notify_store_changed).
//! The app's own consumers (the connected indicator, the Bluetooth settings screen's status line;
//! the passkey card and live catalog in later PRs) read only these app-side types.

/// The radio's link phase, in app vocabulary — what the Bluetooth settings screen's status line
/// shows (P8, #455). Three states, deliberately coarser than the board's own `LinkState`: the UI
/// never needs "stack coming up" (that reads as [`Advertising`](BleLink::Advertising)).
///
/// The connected **indicator** (the title-bar/Home rune, P1) keys on
/// [`Connected`](BleLink::Connected) only — `Off` and `Advertising` both draw nothing there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BleLink {
    /// The radio is disabled (the Bluetooth setting is off): nothing advertises, nothing connects.
    Off,
    /// Powered and unconnected — advertising, connectable. The steady state, and the boot default
    /// until the host feeds the first real snapshot.
    #[default]
    Advertising,
    /// A central holds the (single) link.
    Connected,
}

/// The link state the device UI shows — the whole of what `obc-app` knows about the BLE link.
///
/// Distilled by the host from its radio state (the board's `ble::state` snapshot, or the sim's
/// control panel) and fed in each pass via [`App::set_ble_status`](crate::App::set_ble_status).
/// Deliberately tiny: everything the UI needs, nothing about descriptors, MTUs, or peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BleStatus {
    /// The radio's link phase. [`Connected`](BleLink::Connected) drives the connected indicator
    /// (menu title bar + Home); the full three states drive the Bluetooth screen's status line.
    pub link: BleLink,
    /// The 6-digit LESC passkey to show while pairing, or `None` otherwise. **Plumbed but not yet
    /// consumed** — the passkey card is P2 (#449); until then this rides the seam so the board's
    /// publish path is complete and the sim can inject it.
    pub passkey: Option<u32>,
    /// A bond is stored — the Bluetooth screen's "Paired: yes/no" row (deliberately no phone name).
    /// The board reads its RRAM bond slot; the sim injects it from the control panel.
    pub paired: bool,
}

impl BleStatus {
    /// The powered-but-unlinked default: advertising, no passkey, no bond. The app's boot value
    /// until the host feeds the first real snapshot.
    pub const DISCONNECTED: BleStatus = BleStatus { link: BleLink::Advertising, passkey: None, paired: false };

    /// Whether a central holds the link — the connected indicator's one question.
    pub fn connected(&self) -> bool {
        self.link == BleLink::Connected
    }
}
