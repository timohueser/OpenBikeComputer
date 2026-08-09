//! The **Weather Request** fixtures (`obc-ble-interface-spec.md` §11, WX3 #1188).
//!
//! Three of them are the `weatherRequestContext` value the companion reads before it disconnects
//! again — the one thing the device says about where the rider is and what forecast it already
//! holds. The other two carry the contract's two *append-only* wire changes, and they belong here
//! rather than beside the older identity/Config fixtures because they only mean anything together:
//! a phone that cannot see the capability word never looks for the service, and a phone that cannot
//! write the refresh byte cannot change how often the device asks.
//!
//! Everything below is written from the spec's field table by explicit offset writes. `obc-vectors`
//! does not depend on `obc-ble`, and must not: a vector produced by the very encoder it exists to
//! police would agree with that encoder by construction and pin nothing at all.

// ============================ The layout (spec §11) ============================

/// The context layout version — v1 is the first, so there is no older form to be lenient towards.
pub const CONTEXT_VERSION: u8 = 1;
/// v1's exact encoded length, and the value byte 1 declares.
pub const CONTEXT_ENCODED_LEN: usize = 52;

// Field offsets, straight off the spec's table. Little-endian throughout.
const OFF_VERSION: usize = 0;
const OFF_ENCODED_LEN: usize = 1;
const OFF_VALIDITY: usize = 2;
const OFF_REASON: usize = 4;
const OFF_REFRESH: usize = 6;
const OFF_RESERVED0: usize = 7;
const OFF_REQUEST_ID: usize = 8;
const OFF_LAT: usize = 12;
const OFF_LON: usize = 16;
const OFF_FIX_UTC: usize = 20;
const OFF_BEARING: usize = 28;
const OFF_SPEED: usize = 30;
const OFF_ROUTE_ID: usize = 32;
const OFF_RESERVED1: usize = 34;
const OFF_BUNDLE_GENERATION: usize = 36;
const OFF_BUNDLE_GENERATED_AT: usize = 40;
const OFF_BUNDLE_CRC32: usize = 48;

/// `lat_udeg` / `lon_udeg` / `fix_utc` carry a real fix.
pub const VALID_POSITION: u16 = 1 << 0;
/// `bearing_deg` is a bearing the device trusts.
pub const VALID_BEARING: u16 = 1 << 1;
/// `speed_deci_ms` is a ground speed the device trusts.
pub const VALID_SPEED: u16 = 1 << 2;
/// The three `bundle_*` fields describe a bundle the device holds and has validated.
pub const VALID_BUNDLE: u16 = 1 << 3;
/// `route_id` names the active route object.
pub const VALID_ROUTE: u16 = 1 << 4;

/// The configured interval elapsed mid-ride.
pub const REASON_SCHEDULED: u16 = 1 << 0;
/// The rider opened Weather — fetch now.
pub const REASON_URGENT: u16 = 1 << 1;
/// A step on the retry ladder.
pub const REASON_RETRY: u16 = 1 << 2;
/// Nothing usable on the card (or the held bundle expired).
pub const REASON_NO_BUNDLE: u16 = 1 << 3;
/// The rider left the held bundle's covered corridor.
pub const REASON_OUT_OF_AREA: u16 = 1 << 4;

/// Scheduled refresh **off** — a device that never raises a *scheduled* request, which is not the
/// same as a device that never raises one (see [`no_fix`]).
pub const REFRESH_OFF: u8 = 0;
/// Every 30 minutes — the device default (epic #1185), and what an absent Config field means.
pub const REFRESH_EVERY_30: u8 = 2;
/// Every 60 minutes.
pub const REFRESH_EVERY_60: u8 = 3;

/// The capability bit that says this device implements the whole §11 contract (`feature_bits` in
/// the identity read). One bit covers the service, the context, object type 20 and the Config
/// field, because a phone that has only some of those has nothing it can do.
pub const FEATURE_WEATHER: u32 = 1 << 0;

// ============================ The fixtures' pinned values ============================

/// Freiburg im Breisgau, in the microdegrees the OBCW header uses — inside the OBCW fixtures'
/// bounding box, so the rider in `weather-request-context-full.bin` is genuinely under the forecast
/// the same fixture says they hold. Transposed lat/lon lands in Somalia and is noticed.
pub const FULL_LAT_UDEG: i32 = 47_999_008;
pub const FULL_LON_UDEG: i32 = 7_842_104;
/// The full context's nonce. The high half names the issue that allocated the layout so a value
/// spotted in a log is traceable; the low half is the per-boot counter.
pub const FULL_REQUEST_ID: u32 = 0x1188_0001;
/// The no-fix context's nonce — a *different* request, so a decoder that reads the wrong file is
/// visible rather than merely wrong about the flags.
pub const NO_FIX_REQUEST_ID: u32 = 0x1188_0002;
/// Heading north-north-west at 7.1 m/s (≈ 25.6 km/h): plausible for a loaded touring bike, and
/// neither value is symmetric enough to survive a byte swap unnoticed.
pub const FULL_BEARING_DEG: u16 = 342;
pub const FULL_SPEED_DECI_MS: u16 = 71;
/// The active route is id 7 — the same waypoint route `route-list.bin` catalogs and
/// `status-download-announce.bin` announces, so "the rider is on a route" is a claim the rest of
/// the fixture set can actually back up.
pub const FULL_ROUTE_ID: u16 = 7;
/// The OBCW fixture the full context says the device already holds. The DWD-shaped one, because
/// that is the realistic thing to be riding on: a 96 × 96 raster of native 15-minute frames.
pub const HELD_BUNDLE_FIXTURE: &str = "weather-dwd-96x96-9f.obcw";
/// The store epoch in `version-read-features.bin`, deliberately unlike the `0xA1B2C3D4` the three
/// older identity-read fixtures share: a test that reads the wrong file fails on the epoch rather
/// than silently passing everything except the capability word.
pub const FEATURES_STORE_EPOCH: u32 = 0xC0DE_F00D;
/// The device name in `config-weather-refresh.bin` — different from `config-v1.bin`'s "OBC Tourer"
/// for the same reason, and a different length, so the trailing byte's offset actually moves.
pub const CONFIG_NAME: &str = "OBC Alpine";
/// …and imperial units, so the refresh byte is pinned as sitting *after* a nonzero `units` rather
/// than after a zero that a misaligned reader could mistake for padding.
pub const CONFIG_UNITS: u8 = 1;

// ============================ Builders ============================

/// One context's semantic fields, named after the layout they serialize into. A struct rather than
/// sixteen positional arguments — at this width a positional call is unreviewable, and the whole
/// value of the fixture is that a reader can check it against the spec table by eye.
pub struct ContextFields {
    pub version: u8,
    pub validity: u16,
    pub reason: u16,
    pub refresh: u8,
    pub request_id: u32,
    pub lat_udeg: i32,
    pub lon_udeg: i32,
    pub fix_utc: i64,
    pub bearing_deg: u16,
    pub speed_deci_ms: u16,
    pub route_id: u16,
    pub bundle_generation: u32,
    pub bundle_generated_at: i64,
    pub bundle_crc32: u32,
}

impl ContextFields {
    /// A well-formed v1 value claiming nothing: the resting state of the GATT attribute, which is
    /// what a peer that reads it out of turn must get instead of the last ride's coordinates.
    pub const RESTING: Self = Self {
        version: CONTEXT_VERSION,
        validity: 0,
        reason: 0,
        refresh: REFRESH_EVERY_30,
        request_id: 0,
        lat_udeg: 0,
        lon_udeg: 0,
        fix_utc: 0,
        bearing_deg: 0,
        speed_deci_ms: 0,
        route_id: 0,
        bundle_generation: 0,
        bundle_generated_at: 0,
        bundle_crc32: 0,
    };
}

/// Serialize a context from the spec's field table — every field written at its stated offset, both
/// reserved fields written zero.
pub fn context(f: &ContextFields) -> Vec<u8> {
    let mut b = vec![0u8; CONTEXT_ENCODED_LEN];
    b[OFF_VERSION] = f.version;
    b[OFF_ENCODED_LEN] = CONTEXT_ENCODED_LEN as u8;
    b[OFF_VALIDITY..OFF_VALIDITY + 2].copy_from_slice(&f.validity.to_le_bytes());
    b[OFF_REASON..OFF_REASON + 2].copy_from_slice(&f.reason.to_le_bytes());
    b[OFF_REFRESH] = f.refresh;
    b[OFF_RESERVED0] = 0;
    b[OFF_REQUEST_ID..OFF_REQUEST_ID + 4].copy_from_slice(&f.request_id.to_le_bytes());
    b[OFF_LAT..OFF_LAT + 4].copy_from_slice(&f.lat_udeg.to_le_bytes());
    b[OFF_LON..OFF_LON + 4].copy_from_slice(&f.lon_udeg.to_le_bytes());
    b[OFF_FIX_UTC..OFF_FIX_UTC + 8].copy_from_slice(&f.fix_utc.to_le_bytes());
    b[OFF_BEARING..OFF_BEARING + 2].copy_from_slice(&f.bearing_deg.to_le_bytes());
    b[OFF_SPEED..OFF_SPEED + 2].copy_from_slice(&f.speed_deci_ms.to_le_bytes());
    b[OFF_ROUTE_ID..OFF_ROUTE_ID + 2].copy_from_slice(&f.route_id.to_le_bytes());
    b[OFF_RESERVED1..OFF_RESERVED1 + 2].copy_from_slice(&0u16.to_le_bytes());
    b[OFF_BUNDLE_GENERATION..OFF_BUNDLE_GENERATION + 4].copy_from_slice(&f.bundle_generation.to_le_bytes());
    b[OFF_BUNDLE_GENERATED_AT..OFF_BUNDLE_GENERATED_AT + 8].copy_from_slice(&f.bundle_generated_at.to_le_bytes());
    b[OFF_BUNDLE_CRC32..OFF_BUNDLE_CRC32 + 4].copy_from_slice(&f.bundle_crc32.to_le_bytes());
    b
}

/// `(generation, generated_at, whole-object CRC-32)` of the OBCW bundle the full context says the
/// device holds — **read out of the checked-in bundle fixture** rather than written here as three
/// literals. The point of the `bundle_*` group is that the phone can tell whether the forecast on
/// the card is the one it would have produced, so a fixture whose bundle identity named no real
/// bundle would exercise the field widths and nothing else.
fn held_bundle() -> (u32, i64, u32) {
    use obc_formats::obcw::{HDR_GENERATED_AT, HDR_GENERATION};
    let bytes = super::obcw::dwd_shaped();
    let generation = u32::from_le_bytes(bytes[HDR_GENERATION..HDR_GENERATION + 4].try_into().unwrap());
    let generated_at = i64::from_le_bytes(bytes[HDR_GENERATED_AT..HDR_GENERATED_AT + 8].try_into().unwrap());
    (generation, generated_at, super::crc32(&bytes))
}

/// The **full** context: a rider mid-ride with everything the device can know — a fix, a bearing, a
/// speed, an active route and a validated bundle — asking for the refresh its own 30-minute
/// schedule just came due for.
///
/// The timing is arithmetic rather than decoration: `fix_utc` is exactly 30 minutes after the held
/// bundle's `generated_at`, so `REASON_SCHEDULED` with `REFRESH_EVERY_30` is a claim the numbers in
/// the same file support. Every validity bit is set, which is what makes this the fixture a second
/// implementation is held to: no field is left at zero where a wrong offset could hide.
pub fn full() -> Vec<u8> {
    let (bundle_generation, bundle_generated_at, bundle_crc32) = held_bundle();
    context(&ContextFields {
        validity: VALID_POSITION | VALID_BEARING | VALID_SPEED | VALID_BUNDLE | VALID_ROUTE,
        reason: REASON_SCHEDULED,
        refresh: REFRESH_EVERY_30,
        request_id: FULL_REQUEST_ID,
        lat_udeg: FULL_LAT_UDEG,
        lon_udeg: FULL_LON_UDEG,
        fix_utc: bundle_generated_at + 30 * 60,
        bearing_deg: FULL_BEARING_DEG,
        speed_deci_ms: FULL_SPEED_DECI_MS,
        route_id: FULL_ROUTE_ID,
        bundle_generation,
        bundle_generated_at,
        bundle_crc32,
        ..ContextFields::RESTING
    })
}

/// The **resting** context: nothing is due, nothing is claimed.
///
/// This is the value the characteristic holds between requests, and it exists as a fixture because
/// it is the one every implementation is tempted to skip — an attribute left at all-zeroes would
/// decode as version 0 and refresh `Off`, i.e. as a device that both speaks an unknown layout and
/// has weather switched off, neither of which is true.
pub fn empty() -> Vec<u8> {
    context(&ContextFields::RESTING)
}

/// An **urgent request with nothing behind it**: the rider opened Weather before the receiver has a
/// fix and with no bundle on the card. No validity bits, a nonzero `request_id`, and — the part
/// worth pinning — `refresh` **Off**.
///
/// A request is still a request when the schedule is off: `REFRESH_OFF` configures the *scheduled*
/// interval, not whether the device may ever ask. A phone that gated on the refresh byte would
/// answer every rider except the ones who turned automatic refresh off and then asked by hand.
pub fn no_fix() -> Vec<u8> {
    context(&ContextFields {
        reason: REASON_URGENT | REASON_NO_BUNDLE,
        refresh: REFRESH_OFF,
        request_id: NO_FIX_REQUEST_ID,
        ..ContextFields::RESTING
    })
}

/// The **11-byte** `protocolVersion` read (spec §1): the 7-byte read every current device serves,
/// plus the trailing `feature_bits u32`.
///
/// Composed from [`super::version_read`] on purpose — the capability word is an *append*, and a
/// builder that rewrote the whole layout to add it would stop demonstrating that.
pub fn version_read_features(version: u16, store_epoch: u32, obcm_version: u8, feature_bits: u32) -> Vec<u8> {
    let mut v = super::version_read(version, store_epoch, obcm_version);
    v.extend_from_slice(&feature_bits.to_le_bytes());
    v
}

/// A Config blob carrying the trailing refresh byte (spec §7.3): `name_len u16 · name · units u8 ·
/// weather_refresh u8`.
///
/// The pair with `config-v1.bin` is the fixture: the same object with and without the appended
/// field, so a decoder is pinned on both "an old app's write leaves refresh unspecified" and "a new
/// app's write is read at the right offset", which is a single off-by-one apart.
pub fn config_weather_refresh(name: &str, units: u8, refresh: u8) -> Vec<u8> {
    let name = name.as_bytes();
    let mut v = Vec::with_capacity(2 + name.len() + 2);
    v.extend_from_slice(&(name.len() as u16).to_le_bytes());
    v.extend_from_slice(name);
    v.push(units);
    v.push(refresh);
    v
}
