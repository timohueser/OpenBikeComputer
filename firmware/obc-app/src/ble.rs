//! The host→app BLE **event/state seam** (epic #447, P1): the small app-vocabulary snapshot the
//! host feeds in each pass, plus the store-change signal the object store raises on a commit/delete.
//!
//! `obc-app` stays oblivious to the radio: no `obc-ble` (or board) type crosses this boundary. The
//! host — the board's BLE plane, or the simulator's control panel — distils its link into a
//! [`BleStatus`] and pushes it through [`App::set_ble_status`](crate::App::set_ble_status); the
//! object-store commit/delete paths ring [`App::apply_event`](crate::App::apply_event).
//! The app's own consumers (the connected indicator, the Bluetooth settings screen's status line,
//! the passkey card, the live catalog) read only these app-side types.

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
    /// The 6-digit LESC passkey to show while pairing, or `None` otherwise. Drives the passkey
    /// card (P2, #449): [`App::set_ble_status`](crate::App::set_ble_status) opens a
    /// [`PasskeyScreen`](crate::screen::PasskeyScreen) when this goes `Some` and closes it when it
    /// clears. The board publishes it from the pairing exchange; the sim injects it from the
    /// control panel.
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

/// A GPS position group for the weather request context (WX8, #1193; spec §11.4 validity bit 0).
/// All three fields travel together because the spec guards them with **one** validity bit: a fix
/// the app can't date (no trusted clock this boot) is not served as a fix at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherFix {
    /// WGS84 latitude, microdegrees.
    pub lat_udeg: i32,
    /// WGS84 longitude, microdegrees.
    pub lon_udeg: i32,
    /// UTC seconds of the fix — the wall clock read back to when the fix arrived.
    pub fix_utc: i64,
}

/// The app-side half of the §11.4 weather request context (WX8, #1193): everything the *app* knows
/// that the request context carries — position, travel bearing/speed, the active route's durable
/// id, ride state, and the trusted-clock "now". Distilled by
/// [`App::weather_snapshot`](crate::App::weather_snapshot) and pushed across the plane seam each
/// pass, the reverse direction of [`BleStatus`]; like it, **no `obc-ble` type crosses here** — the
/// board's weather plane maps these onto the wire layout and its validity bits.
///
/// Every field is optional-by-honesty: `None` means the group is *absent* (the spec's
/// flags-not-sentinels rule), never a zero the peer could mistake for the equator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeatherSnapshot {
    /// A ride is being tracked — the scheduler's "scheduled requests only while riding" gate.
    pub ride_active: bool,
    /// The last GPS fix, only while fresh **and** datable (trusted clock).
    pub position: Option<WeatherFix>,
    /// Travel bearing in whole degrees `0..=359` — the GPS course, only while actually moving
    /// (a stationary receiver's course is noise, not a bearing the device believes).
    pub bearing_deg: Option<u16>,
    /// Ground speed in 0.1 m/s, from the same fresh fix.
    pub speed_deci_ms: Option<u16>,
    /// The active route's **durable object id** — the id the phone's route list knows.
    pub route_id: Option<u16>,
    /// UTC unix seconds now, only when the clock was established from a real source this boot
    /// ([`App::clock_trusted`](crate::App::clock_trusted)) — the scheduler's bundle-age input.
    pub now_utc: Option<u32>,
}
