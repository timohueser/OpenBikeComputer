//! The **Weather Request** contract (spec §11): the dedicated secondary service a disconnected
//! device advertises while a weather refresh is due, and the one small authenticated read the
//! companion performs before it disconnects again.
//!
//! The shape of the exchange is what makes it cheap enough to run on a phone's background budget:
//!
//! 1. The device raises a request and swaps its **advertised** service UUID from OBC Control to
//!    [`WEATHER_REQUEST_SERVICE_UUID`]. Both services exist in GATT at all times — advertising a
//!    service the connected database does not contain is exactly the trap this crate refuses to set.
//! 2. iOS wakes on the service match, connects, and reads one
//!    [`WeatherRequestContext`] — *where the rider is, where they are heading, and what bundle they
//!    already have*. Then it disconnects. BLE is not held across the HTTP that follows.
//! 3. The phone builds an OBCW bundle and uploads it as [`ObjectType::WeatherBundle`]
//!    (`crate::descriptor`) over the ordinary reliable CoC, stamping
//!    [`WeatherRequestContext::request_id`] into the OBCW header's `request_id` field so device and
//!    phone can correlate the two connections.
//!
//! Everything here is pure codec + policy: no radio, no clock, no storage. The board crate owns the
//! GATT table and the advertising switch; the companion mirrors these layouts field-for-field and
//! `specs/vectors/weather-request-*.bin` pins both.
//!
//! ## What the request id is, and what it is not
//!
//! It correlates a request with the bundle produced for it. It is **not** an authorisation token
//! and **not** an upload gate: a bundle that arrives carrying a stale (or unknown) request id is
//! still accepted if it validates and is newer than the active one, because a fresher forecast is
//! useful no matter which request provoked it. See [`UploadDisposition`].

use crate::descriptor::DescriptorError;

/// The dedicated OBC **Weather Request** service.
///
/// A random 128-bit base of its own, allocated by WX3 (#1188) — deliberately *not* a block inside
/// the OBC Control base, because iOS matches the advertisement on this UUID alone and the two
/// services must be independently advertisable. The display form is what specs and CoreBluetooth
/// use; the little-endian form is the byte order the advertising AD structure carries on air.
///
/// This base has never shipped in a released firmware, so `0001` below is a first assignment and
/// not a reuse. No block of this base is retired.
pub const WEATHER_REQUEST_SERVICE_UUID: &str = "B3B60000-33B4-4F02-A5FF-E5954D54B5AA";
/// [`WEATHER_REQUEST_SERVICE_UUID`] in advertising byte order (least-significant byte first).
pub const WEATHER_REQUEST_SERVICE_UUID_LE: [u8; 16] =
    [0xAA, 0xB5, 0x54, 0x4D, 0x95, 0xE5, 0xFF, 0xA5, 0x02, 0x4F, 0xB4, 0x33, 0x00, 0x00, 0xB6, 0xB3];

/// The read-only, **authenticated** request-context characteristic of the Weather Request service.
///
/// Authenticated because the value describes where the rider is. An unbonded peer that connects to
/// the advertisement gets an ATT security error, and — see [`authenticated_context_was_served`] —
/// does not consume the pending request either.
pub const WEATHER_REQUEST_CONTEXT_UUID: &str = "B3B60001-33B4-4F02-A5FF-E5954D54B5AA";

/// The context layout version. v1 is exactly [`WeatherRequestContext::ENCODED_LEN`] bytes.
pub const WEATHER_REQUEST_CONTEXT_VERSION: u8 = 1;

/// The one object id a [`ObjectType::WeatherBundle`](crate::descriptor::ObjectType::WeatherBundle)
/// upload may target. There is exactly one weather bundle, so the id selects nothing — any other
/// value is answered `notFound` rather than quietly treated as this one.
pub const WEATHER_BUNDLE_OBJECT_ID: u16 = 0;

// ============================ Validity flags ============================

/// `lat_udeg` / `lon_udeg` / `fix_utc` carry a real GPS fix.
pub const VALID_POSITION: u16 = 1 << 0;
/// `bearing_deg` is a trustworthy travel bearing (moving, with a course the device believes).
pub const VALID_BEARING: u16 = 1 << 1;
/// `speed_deci_ms` is a trustworthy ground speed.
pub const VALID_SPEED: u16 = 1 << 2;
/// `bundle_generation` / `bundle_generated_at` / `bundle_crc32` describe a bundle the device has
/// validated and selected. Clear means *no usable bundle on the card* — not "generation 0".
pub const VALID_BUNDLE: u16 = 1 << 3;
/// `route_id` names the active route object.
pub const VALID_ROUTE: u16 = 1 << 4;

// ============================ Reason flags ============================

/// The configured refresh interval elapsed during an active ride.
pub const REASON_SCHEDULED: u16 = 1 << 0;
/// The rider opened Weather — the phone should treat this as urgent.
pub const REASON_URGENT: u16 = 1 << 1;
/// A previous attempt failed; this is a step on the 5/10/20-minute retry ladder.
pub const REASON_RETRY: u16 = 1 << 2;
/// There is no usable bundle at all, or the active one has expired.
pub const REASON_NO_BUNDLE: u16 = 1 << 3;
/// The rider has travelled outside the active bundle's covered corridor.
pub const REASON_OUT_OF_AREA: u16 = 1 << 4;

/// How often the device raises a scheduled request. Appended to Config (§7.3) and echoed in the
/// request context so the phone can schedule its own work without a second read.
///
/// The wire is the discriminant; the *minutes* are derived, so `Off` needs no sentinel minute value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WeatherRefresh {
    Off = 0,
    Every15 = 1,
    Every30 = 2,
    Every60 = 3,
    Every120 = 4,
}

impl WeatherRefresh {
    /// The device default (epic #1185: 30 minutes) — also what an absent Config field means.
    pub const DEFAULT: Self = Self::Every30;

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Decode a refresh byte. An unknown value is **not** an error the caller can paper over with a
    /// default: it means the peer configured an interval this build does not know, and silently
    /// substituting 30 minutes would misreport the device's own setting back to the rider.
    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            0 => Self::Off,
            1 => Self::Every15,
            2 => Self::Every30,
            3 => Self::Every60,
            4 => Self::Every120,
            other => return Err(DescriptorError::UnknownStatus(other)),
        })
    }

    /// The interval in minutes; `None` for [`Off`](Self::Off), which has no interval at all.
    pub const fn minutes(self) -> Option<u16> {
        match self {
            Self::Off => None,
            Self::Every15 => Some(15),
            Self::Every30 => Some(30),
            Self::Every60 => Some(60),
            Self::Every120 => Some(120),
        }
    }
}

// ============================ The request context ============================

/// The one value the companion reads before it disconnects — 52 little-endian bytes describing the
/// request and the rider.
///
/// Optional groups are guarded by [`validity`](Self::validity) bits rather than by sentinel values,
/// so "no fix" is *absent* rather than the equator, and "no bundle" is *absent* rather than
/// generation 0. Fields mirror the OBCW header's widths exactly (`i32` microdegrees, `i64` UTC
/// seconds, `u32` generation/CRC) so a value round-trips into a bundle header without narrowing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeatherRequestContext {
    /// Layout version — [`WEATHER_REQUEST_CONTEXT_VERSION`] for anything this build writes.
    pub version: u8,
    /// Which optional groups below are populated. Unknown bits are ignored, never rejected.
    pub validity: u16,
    /// Why this request is due — advisory scheduling help for the phone, never an upload gate. A
    /// phone that recognises none of the bits still performs the full fetch.
    pub reason: u16,
    /// The device's configured refresh interval, so the phone need not read Config too.
    pub refresh: WeatherRefresh,
    /// The request nonce, echoed into the OBCW header's `request_id`. Monotonic per device boot;
    /// **stable across the retry ladder** so retries of one request stay one request.
    pub request_id: u32,
    /// WGS84 latitude in microdegrees ([`VALID_POSITION`]).
    pub lat_udeg: i32,
    /// WGS84 longitude in microdegrees ([`VALID_POSITION`]).
    pub lon_udeg: i32,
    /// UTC seconds of that fix ([`VALID_POSITION`]).
    pub fix_utc: i64,
    /// Travel bearing, whole degrees `0..=359` ([`VALID_BEARING`]).
    pub bearing_deg: u16,
    /// Ground speed in tenths of a metre per second ([`VALID_SPEED`]).
    pub speed_deci_ms: u16,
    /// The active route's object id ([`VALID_ROUTE`]).
    pub route_id: u16,
    /// Active bundle generation ([`VALID_BUNDLE`]).
    pub bundle_generation: u32,
    /// Active bundle `generated_at`, UTC seconds ([`VALID_BUNDLE`]).
    pub bundle_generated_at: i64,
    /// Active bundle whole-object CRC-32 ([`VALID_BUNDLE`]).
    pub bundle_crc32: u32,
}

impl WeatherRequestContext {
    /// v1's exact encoded length, and the value byte 1 carries.
    pub const ENCODED_LEN: usize = 52;
    /// The shortest read that can be decoded at all: the version/length prefix itself.
    pub const MIN_ENCODED: usize = 2;

    // Field offsets — the single source both `encode` and `decode` index through, so the two can
    // never drift apart in a way the round-trip test would have to catch.
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

    /// An empty context: a well-formed v1 value with nothing valid and no reason. This is what the
    /// GATT attribute holds before any request is raised, so a peer that reads the characteristic
    /// out of turn gets a structurally valid "nothing is due" rather than stale rider coordinates.
    pub const EMPTY: Self = Self {
        version: WEATHER_REQUEST_CONTEXT_VERSION,
        validity: 0,
        reason: 0,
        refresh: WeatherRefresh::DEFAULT,
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

    pub const fn has(&self, flag: u16) -> bool {
        self.validity & flag != 0
    }

    pub const fn because(&self, flag: u16) -> bool {
        self.reason & flag != 0
    }

    /// Encode the fixed 52-byte v1 value. Reserved bytes are written zero; see [`decode`](Self::decode)
    /// for why readers ignore rather than reject them.
    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[Self::OFF_VERSION] = self.version;
        b[Self::OFF_ENCODED_LEN] = Self::ENCODED_LEN as u8;
        b[Self::OFF_VALIDITY..Self::OFF_VALIDITY + 2].copy_from_slice(&self.validity.to_le_bytes());
        b[Self::OFF_REASON..Self::OFF_REASON + 2].copy_from_slice(&self.reason.to_le_bytes());
        b[Self::OFF_REFRESH] = self.refresh.as_u8();
        b[Self::OFF_RESERVED0] = 0;
        b[Self::OFF_REQUEST_ID..Self::OFF_REQUEST_ID + 4].copy_from_slice(&self.request_id.to_le_bytes());
        b[Self::OFF_LAT..Self::OFF_LAT + 4].copy_from_slice(&self.lat_udeg.to_le_bytes());
        b[Self::OFF_LON..Self::OFF_LON + 4].copy_from_slice(&self.lon_udeg.to_le_bytes());
        b[Self::OFF_FIX_UTC..Self::OFF_FIX_UTC + 8].copy_from_slice(&self.fix_utc.to_le_bytes());
        b[Self::OFF_BEARING..Self::OFF_BEARING + 2].copy_from_slice(&self.bearing_deg.to_le_bytes());
        b[Self::OFF_SPEED..Self::OFF_SPEED + 2].copy_from_slice(&self.speed_deci_ms.to_le_bytes());
        b[Self::OFF_ROUTE_ID..Self::OFF_ROUTE_ID + 2].copy_from_slice(&self.route_id.to_le_bytes());
        b[Self::OFF_RESERVED1..Self::OFF_RESERVED1 + 2].copy_from_slice(&0u16.to_le_bytes());
        b[Self::OFF_BUNDLE_GENERATION..Self::OFF_BUNDLE_GENERATION + 4]
            .copy_from_slice(&self.bundle_generation.to_le_bytes());
        b[Self::OFF_BUNDLE_GENERATED_AT..Self::OFF_BUNDLE_GENERATED_AT + 8]
            .copy_from_slice(&self.bundle_generated_at.to_le_bytes());
        b[Self::OFF_BUNDLE_CRC32..Self::OFF_BUNDLE_CRC32 + 4].copy_from_slice(&self.bundle_crc32.to_le_bytes());
        b
    }

    /// Decode a context read.
    ///
    /// The read is **length-declared**: byte 1 states how many bytes the writer produced, and a read
    /// that delivers fewer than that is [`Truncated`](DescriptorError::Truncated) rather than
    /// half-decoded. Bytes past this version's 52 are ignored, so a future firmware that appends a
    /// field keeps working against a shipped app — the same append-only rule the identity read and
    /// Config live under.
    ///
    /// Reserved bytes and unknown validity/reason bits are **ignored, not rejected** (the deliberate
    /// difference from OBCW's header, which rejects nonzero reserved): those bits are how a later
    /// firmware says something this build was never going to act on, and refusing the whole read
    /// over one would strand a rider's forecast on a byte nobody needed.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::MIN_ENCODED {
            return Err(DescriptorError::Truncated);
        }
        let declared = data[Self::OFF_ENCODED_LEN] as usize;
        if declared < Self::ENCODED_LEN || data.len() < declared {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self {
            version: data[Self::OFF_VERSION],
            validity: rd_u16(data, Self::OFF_VALIDITY),
            reason: rd_u16(data, Self::OFF_REASON),
            refresh: WeatherRefresh::from_u8(data[Self::OFF_REFRESH])?,
            request_id: rd_u32(data, Self::OFF_REQUEST_ID),
            lat_udeg: rd_u32(data, Self::OFF_LAT) as i32,
            lon_udeg: rd_u32(data, Self::OFF_LON) as i32,
            fix_utc: rd_u64(data, Self::OFF_FIX_UTC) as i64,
            bearing_deg: rd_u16(data, Self::OFF_BEARING),
            speed_deci_ms: rd_u16(data, Self::OFF_SPEED),
            route_id: rd_u16(data, Self::OFF_ROUTE_ID),
            bundle_generation: rd_u32(data, Self::OFF_BUNDLE_GENERATION),
            bundle_generated_at: rd_u64(data, Self::OFF_BUNDLE_GENERATED_AT) as i64,
            bundle_crc32: rd_u32(data, Self::OFF_BUNDLE_CRC32),
        })
    }
}

fn rd_u16(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}

fn rd_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

fn rd_u64(d: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3], d[off + 4], d[off + 5], d[off + 6], d[off + 7]])
}

// ============================ Upload disposition ============================

/// What the device does with an arriving weather bundle that has already passed CRC and OBCW
/// structural validation.
///
/// The rule is *newest valid generation wins*, and it is deliberately independent of the request
/// id — see the module docs. Serial arithmetic (RFC-1982 style, as `obc_weather::slots`) decides
/// "newer", so a generation counter that wraps does not strand the device on a bundle from before
/// the wrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadDisposition {
    /// Commit into the inactive slot and select it. The only outcome that changes what the rider sees.
    Commit,
    /// Structurally fine and already held: the same generation is on the card. Reported as
    /// **success**, because the phone did nothing wrong and retrying would not improve matters.
    DuplicateIgnored,
    /// The upload is older than the active bundle. Also **success** — a device that answered an
    /// error here would push a phone with a slow HTTP path into an unwinnable retry loop.
    StaleIgnored,
}

/// What identifies one bundle against another for selection purposes: the pair
/// `obc_weather::slots::Candidate` compares on. Both halves are needed — see [`classify_upload`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundleIdentity {
    pub generation: u32,
    /// The OBCW header's `generated_at`, UTC seconds.
    pub generated_at: i64,
}

/// Decide an arriving bundle's fate. `active` is `None` when no valid bundle is held, in which case
/// anything that validated is an improvement.
///
/// **This must agree with `obc_weather::slots::candidate_is_newer` exactly**, and the test suite
/// pins that agreement across the whole matrix rather than trusting this comment — an earlier draft
/// compared generations alone and silently disagreed with the storage layer in two places, which is
/// the sort of divergence that shows up as "the device kept the old forecast" and nothing else.
///
/// The rule, in full: serial (RFC-1982 style) arithmetic on the generation decides, *except* when
/// the two are equal or exactly half the range apart — both genuinely ambiguous — where the later
/// `generated_at` wins. An exact tie on both is not a replacement.
pub fn classify_upload(incoming: BundleIdentity, active: Option<BundleIdentity>) -> UploadDisposition {
    let Some(active) = active else {
        return UploadDisposition::Commit;
    };
    let delta = incoming.generation.wrapping_sub(active.generation);
    let newer = if delta == 0 || delta == 0x8000_0000 {
        incoming.generated_at > active.generated_at
    } else {
        delta < 0x8000_0000
    };
    if newer {
        UploadDisposition::Commit
    } else if incoming == active {
        UploadDisposition::DuplicateIgnored
    } else {
        UploadDisposition::StaleIgnored
    }
}

// ============================ Advertising policy ============================

/// Whether serving this read may lower the pending advertising hint.
///
/// Both halves matter. A connection that never authenticated must not consume the request — that
/// is how a passer-by's scan would silently cost the rider a forecast — and neither must a read
/// whose response never reached the controller.
pub const fn authenticated_context_was_served(link_secured: bool, reply_sent: bool) -> bool {
    link_secured && reply_sent
}

/// The monotonic total advertising budget for one weather request, in the caller's clock ticks.
///
/// Keeping the **original** deadline rather than restarting a timer per connection is the point: a
/// stray central that connects and drops repeatedly would otherwise extend a bounded request hint
/// indefinitely, turning a 60-second window into a permanent secondary beacon and a battery bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeatherRequestBudget {
    deadline_ticks: u64,
}

impl WeatherRequestBudget {
    pub const fn new(now_ticks: u64, window_ticks: u64) -> Self {
        Self { deadline_ticks: now_ticks.saturating_add(window_ticks) }
    }

    pub const fn remaining_ticks(self, now_ticks: u64) -> u64 {
        self.deadline_ticks.saturating_sub(now_ticks)
    }

    pub const fn expired(self, now_ticks: u64) -> bool {
        self.remaining_ticks(now_ticks) == 0
    }
}
