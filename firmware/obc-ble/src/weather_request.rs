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
/// The held bundle has no rain frames. The phone must perform a full build rather than concluding
/// that an unchanged rain-manifest generation means this hourly-only bundle is complete.
pub const REASON_HOURLY_ONLY: u16 = 1 << 5;

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

    /// Decode a refresh byte. An unknown value is never silently substituted with the default —
    /// that would misreport the peer's own setting — but what a caller *does* with the error is
    /// **direction-dependent**, and that asymmetry is the whole rule (spec §11.8):
    ///
    /// - **Phone → device (a Config write)** is the one direction that must **reject**. The device
    ///   is being asked to adopt an interval it cannot honour, and storing anything else would tell
    ///   the rider their choice was applied when it was discarded. See
    ///   [`Config::refresh_to_apply`].
    /// - **Device → phone** (the context read, a Config read) must **tolerate**: an unknown value
    ///   there is a *newer firmware naming an interval this build predates*. Treating it as fatal
    ///   would mean that appending a fifth interval — an ordinary enum append — silently killed
    ///   weather on every shipped app, and locked it out of Config badly enough that it could no
    ///   longer even rename the device. Those readers take [`Config::known_refresh`] /
    ///   [`WeatherRequestContext::refresh`], which report *unknown* exactly as an unrecognised
    ///   `reason` bit is reported: ignored, not fatal.
    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            0 => Self::Off,
            1 => Self::Every15,
            2 => Self::Every30,
            3 => Self::Every60,
            4 => Self::Every120,
            other => return Err(DescriptorError::UnknownRefresh(other)),
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
    /// The device's configured refresh interval, **as the byte arrived**, so the phone need not
    /// read Config too. Raw for the same reason [`validity`](Self::validity) and
    /// [`reason`](Self::reason) are raw words: this is a device → phone read, so a value this build
    /// does not know is a *newer firmware*, not a malformed one. Use [`refresh`](Self::refresh) for
    /// the typed view and keep this for a verbatim round-trip. Spec §11.8.
    pub refresh_raw: u8,
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
        refresh_raw: WeatherRefresh::DEFAULT.as_u8(),
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

    /// The **resting** value the GATT attribute holds whenever no request is pending: structurally
    /// [`EMPTY`](Self::EMPTY) — nothing valid, no reason, request id 0 — but carrying the device's
    /// *stored* refresh byte rather than the compile-time default. §11.8's whole point is that the
    /// refresh byte reports the rider's own setting; a resting value that said "30 min" over a
    /// persisted `Off` would misreport it from boot until the first raise (#1221 F2).
    pub const fn resting(refresh_raw: u8) -> Self {
        Self { refresh_raw, ..Self::EMPTY }
    }

    pub const fn has(&self, flag: u16) -> bool {
        self.validity & flag != 0
    }

    pub const fn because(&self, flag: u16) -> bool {
        self.reason & flag != 0
    }

    /// The configured refresh interval, or `None` when the device named one this build does not
    /// know — the read direction of §11.8. `None` here means *unknown*, not `Off` and not the
    /// default: a phone that collapsed it to either would misreport the rider's own setting back
    /// to them.
    pub const fn refresh(&self) -> Option<WeatherRefresh> {
        match WeatherRefresh::from_u8(self.refresh_raw) {
            Ok(refresh) => Some(refresh),
            Err(_) => None,
        }
    }

    /// Encode the fixed 52-byte v1 value. Reserved bytes are written zero; see [`decode`](Self::decode)
    /// for why readers ignore rather than reject them.
    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[Self::OFF_VERSION] = self.version;
        b[Self::OFF_ENCODED_LEN] = Self::ENCODED_LEN as u8;
        b[Self::OFF_VALIDITY..Self::OFF_VALIDITY + 2].copy_from_slice(&self.validity.to_le_bytes());
        b[Self::OFF_REASON..Self::OFF_REASON + 2].copy_from_slice(&self.reason.to_le_bytes());
        b[Self::OFF_REFRESH] = self.refresh_raw;
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
            // Never rejected: see `refresh_raw`. A byte this build does not know rides through
            // verbatim and reads as `None` from `refresh()`.
            refresh_raw: data[Self::OFF_REFRESH],
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

// ============================ The due scheduler ============================

/// The failure retry ladder, in seconds (spec §11.3: **5 / 10 / 20 minutes**): the waits between
/// successive advertising raises of one pending request. Past the last rung a **scheduled**
/// request's wait becomes the configured refresh cadence — "capped by the configured interval"
/// (epic #1185) — while an **urgent** request always lapses after the final rung, cadence or not
/// (locked in #1221's review round): the rider's tap earns three fast retries, never a standing
/// beacon, and reopening Weather raises a fresh request. A scheduled request under `Off` lapses
/// the same way (nothing configures a cadence to fall back to).
pub const RETRY_LADDER_S: [u64; 3] = [5 * 60, 10 * 60, 20 * 60];
/// How long the final urgent raise remains consumable before the request lapses. Kept equal to the
/// board advertising budget: a raise returned by `poll` must remain pending for the interval in
/// which the companion can actually discover and read it.
pub const WEATHER_REQUEST_WINDOW_S: u64 = 60;

/// When a held bundle stops counting as one for the `reason` word: OBCW v1 carries 24 hourly
/// records, so a bundle a day old has nothing left to say and the request advertises
/// [`REASON_NO_BUNDLE`] — *"or the active one has expired"* (§11.4). Display staleness is the
/// screens' own, stricter judgement (WX10/WX11); this constant only shapes the advisory reason
/// bits, never whether the bundle stays readable.
pub const BUNDLE_EXPIRED_S: u64 = 24 * 3600;

/// What the scheduler knows about the active bundle at a poll, distilled by the caller from the
/// slot selection + the trusted clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BundleFacts {
    /// A validated bundle is selected on the card (regardless of age — expired stays readable).
    pub held: bool,
    /// Its age in seconds — only when both a bundle and a trusted clock exist.
    pub age_s: Option<u64>,
    /// The held bundle is both before the next possible service publication and within the manual
    /// location reuse radius. Opening Weather can use it without waking the phone.
    pub manual_reusable: bool,
    /// A fresh fix is outside the location reuse radius around the held bundle.
    pub location_changed: bool,
    /// The selected bundle contains no rain frames.
    pub hourly_only: bool,
}

impl BundleFacts {
    /// No bundle at all.
    pub const NONE: Self =
        Self { held: false, age_s: None, manual_reusable: false, location_changed: false, hourly_only: false };

    /// Whether the bundle counts as *usable* for the reason word (held and not expired). Unknown
    /// age is treated as usable: stale/no-data must never be *invented*, and a device without a
    /// clock cannot claim expiry it can't measure.
    pub const fn usable(self) -> bool {
        self.held
            && match self.age_s {
                Some(age) => age < BUNDLE_EXPIRED_S,
                None => true,
            }
    }
}

/// One advertising raise the scheduler asks the board to perform: fill the request context, then
/// arm the Weather Request advertising hint for its bounded window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raise {
    /// §11.2 — monotonic per boot, **stable across the retry ladder**.
    pub request_id: u32,
    /// The §11.4 advisory reason word for the context.
    pub reason: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    request_id: u32,
    reason: u16,
    /// When the next ladder re-raise fires (absolute, caller seconds). Always meaningful while the
    /// pending is stored. For the final urgent raise it is the lapse deadline, leaving that raise
    /// consumable for [`WEATHER_REQUEST_WINDOW_S`] instead of invalidating it in the same poll.
    next_raise_s: u64,
    /// Raises performed so far (1 = the initial raise).
    raises: u8,
    /// The ladder is exhausted; `next_raise_s` is now the lapse instant, not another raise.
    final_raise: bool,
}

/// The **due scheduler** (WX8, #1193): decides *when* a weather request is raised, entirely as a
/// pure clock-driven state machine — no radio, no storage, no timer of its own, so the whole
/// ride/urgent/retry/commit matrix is pinned by host tests against a synthetic clock.
///
/// The caller (the board's weather plane, later the simulator's) owns the loop: it feeds
/// [`poll`](Self::poll) the current monotonic seconds and the current *levels* (refresh setting,
/// ride state, bundle facts), performs the returned [`Raise`] (context fill + advertising arm),
/// and sleeps until [`next_wake_s`](Self::next_wake_s) or an event edge.
///
/// The rules, from the epic + spec §11 (+ the two decisions locked in #1221's review round):
/// - **Scheduled requests only while a ride is active**; a pending scheduled request is dropped the
///   moment the ride stops (or the cadence is set `Off`). Urgent requests survive both.
/// - **Opening Weather is urgent** ([`open_weather`](Self::open_weather)) — raised immediately,
///   even outside a ride, even with refresh `Off`.
/// - **No storage, no requests** (`store_ready`): a card-less device would answer every upload
///   `error`, which is exactly §11.7's phone-burning loop — so it never advertises a request at
///   all, urgent included, and a pending request is dropped the moment the card goes away.
/// - Due state derives from the last **accepted upload** this boot, or — across a reboot — from
///   the active bundle's age (no countdown is ever persisted). A held bundle whose age is unknown
///   (no trusted clock) anchors at scheduler start: without a clock the device cannot claim the
///   interval already elapsed. The anchor paces even an *unusable* (expired) bundle: an accepted
///   answer means "nothing newer exists upstream", and re-asking a second later would loop.
/// - Failure retries ladder at [`RETRY_LADDER_S`]; every re-raise keeps the **same** `request_id`
///   (§11.2) and adds [`REASON_RETRY`]. Past the ladder a scheduled request falls back to the
///   configured cadence; an **urgent request always lapses** (three fast retries, never a
///   standing beacon).
/// - **Any accepted upload finishes a request** ([`commit_succeeded`](Self::commit_succeeded)) —
///   fresh, duplicate, or stale: §11.6 answers all three `committed`, and each is the phone's
///   complete answer. A served context read or an expired advertising window does not finish one
///   (§11.3 — the advertising budget is the board's, not this scheduler's).
pub struct DueScheduler {
    /// §11.2: the next request id this boot (starts at 1 — id 0 is the resting context's).
    next_request_id: u32,
    pending: Option<Pending>,
    /// Monotonic second of the last successful commit this boot.
    last_commit_s: Option<u64>,
    /// The reboot anchor: where the interval countdown stands, reconstructed once from the active
    /// bundle's age (or the scheduler's own start when the age is unknowable). Superseded by
    /// `last_commit_s` the moment a commit lands. Signed because a bundle can be older than the
    /// monotonic clock is — an anchor before boot is exactly what makes it *past due* at ride start.
    boot_anchor_s: Option<i64>,
    /// The scheduler's first-poll instant — the age-unknown fallback anchor.
    started_s: Option<u64>,
    /// A queued urgent request (the rider opened Weather), consumed by the next poll.
    urgent_queued: bool,
    /// A successful phone-side conditional check can prove the held bytes are still current without
    /// uploading them. Until this monotonic instant, an urgent reopen reuses the held bundle.
    source_defer_until_s: Option<u64>,
}

impl DueScheduler {
    pub const fn new() -> Self {
        Self {
            next_request_id: 1,
            pending: None,
            last_commit_s: None,
            boot_anchor_s: None,
            started_s: None,
            urgent_queued: false,
            source_defer_until_s: None,
        }
    }

    /// The rider opened Weather: queue an urgent raise for the next poll. If a request is already
    /// pending the raise re-uses its id (one request, not parallel jobs) with a fresh fast ladder.
    pub fn open_weather(&mut self) {
        self.urgent_queued = true;
    }

    /// A weather upload was **accepted** — fresh (`commit`), duplicate, or stale, all of which the
    /// wire answers `committed` (§11.6): clear the pending request and anchor the next scheduled
    /// interval here. The duplicate/stale rows count deliberately (#1221 F3): they are the phone's
    /// complete answer — "nothing newer exists upstream" — and treating them as anything less
    /// would re-raise the same request against the same upstream a second later, forever.
    pub fn commit_succeeded(&mut self, now_s: u64) {
        self.pending = None;
        self.last_commit_s = Some(now_s);
        self.boot_anchor_s = None;
        self.source_defer_until_s = None;
    }

    /// The phone conditionally checked both weather sources and found no newer data. This is the
    /// small-payload twin of an accepted upload: it finishes exactly the request it names and
    /// anchors normal scheduled pacing, while `retry_after_s` prevents repeated manual opens from
    /// probing through the publisher's processing lag.
    pub fn unchanged_succeeded(&mut self, request_id: u32, now_s: u64, retry_after_s: u16) -> bool {
        if self.pending_request_id() != Some(request_id) {
            return false;
        }
        self.pending = None;
        self.last_commit_s = Some(now_s);
        self.boot_anchor_s = None;
        self.source_defer_until_s = Some(now_s.saturating_add(retry_after_s as u64));
        true
    }

    /// Whether a request is currently pending (raised and not yet satisfied).
    pub fn pending_request_id(&self) -> Option<u32> {
        self.pending.map(|p| p.request_id)
    }

    /// Advance the machine to `now_s` under the current levels and return the advertising raise to
    /// perform, if one is due. At most one raise per poll; a caller that sleeps until
    /// [`next_wake_s`](Self::next_wake_s) never misses one, only delays it.
    ///
    /// `store_ready` is "the device can accept a bundle right now" (a mounted card): with it false
    /// **nothing** raises — see the type docs for why that is §11.7's rule, not caution.
    pub fn poll(
        &mut self,
        now_s: u64,
        refresh: WeatherRefresh,
        ride_active: bool,
        store_ready: bool,
        bundle: BundleFacts,
    ) -> Option<Raise> {
        if self.started_s.is_none() {
            self.started_s = Some(now_s);
        }
        // No storage ⇒ no requests of any kind (#1221 F5): every upload would be answered `error`,
        // so advertising a request would send the phone round §11.7's fetch-build-upload loop at
        // its own expense, forever. A pending request is dropped (not parked): the card that left
        // took the request's meaning with it, and a re-insert raises a fresh one through the
        // normal machinery. A queued urgent tap is consumed too — reopening Weather after
        // inserting a card is the honest re-arm.
        if !store_ready {
            self.pending = None;
            self.urgent_queued = false;
            return None;
        }
        // Seed the reboot anchor once: a bundle whose age is known counts as "satisfied age_s ago";
        // one with unknown age (no trusted clock) counts from scheduler start.
        if self.boot_anchor_s.is_none() && self.last_commit_s.is_none() && bundle.held {
            self.boot_anchor_s = Some(match bundle.age_s {
                Some(age) => now_s as i64 - age as i64,
                None => now_s as i64,
            });
        }

        // A pending *scheduled* request lapses when its preconditions do: the ride stopped, or the
        // cadence was set Off. Urgent requests survive both (the rider asked, the answer is still
        // wanted).
        if let Some(p) = self.pending {
            let urgent = p.reason & REASON_URGENT != 0;
            if !urgent && (!ride_active || refresh.minutes().is_none()) {
                self.pending = None;
            }
        }

        let no_bundle_bit = if bundle.usable() { 0 } else { REASON_NO_BUNDLE };
        let location_bit = if bundle.location_changed { REASON_OUT_OF_AREA } else { 0 };
        let hourly_only_bit = if bundle.hourly_only { REASON_HOURLY_ONLY } else { 0 };

        // Urgent: raise immediately — re-using a pending request's id (one request, fresh fast
        // ladder), or minting a new one.
        if core::mem::take(&mut self.urgent_queued) {
            let check_still_current = self.source_defer_until_s.is_some_and(|until| now_s < until);
            if bundle.held && !bundle.location_changed && (bundle.manual_reusable || check_still_current) {
                return None;
            }
            let (request_id, prior_reason) = match self.pending {
                Some(p) => (p.request_id, p.reason),
                None => (self.mint_id(), 0),
            };
            let reason = prior_reason | REASON_URGENT | no_bundle_bit | location_bit | hourly_only_bit;
            self.pending = Some(Pending {
                request_id,
                reason,
                next_raise_s: now_s + RETRY_LADDER_S[0],
                raises: 1,
                final_raise: false,
            });
            return Some(Raise { request_id, reason });
        }

        // The retry ladder: re-raise the pending request with the same id (§11.2), stepping the
        // wait through §11.3's rungs; past them a scheduled request paces at the configured
        // cadence, while an urgent one has no fallback at all (#1221 F4).
        if let Some(p) = &mut self.pending {
            if now_s < p.next_raise_s {
                return None;
            }
            if p.final_raise {
                self.pending = None;
                return None;
            }
            p.raises = p.raises.saturating_add(1);
            p.reason |= REASON_RETRY | no_bundle_bit | location_bit | hourly_only_bit;
            let raise = Raise { request_id: p.request_id, reason: p.reason };
            // Waits between raises: the initial raise armed rung 0 (5 min); retry `k` arms rung
            // `k` (10, 20), and past the last rung the cadence — or nothing.
            let next_wait = match RETRY_LADDER_S.get(p.raises as usize - 1) {
                Some(&rung) => Some(rung),
                None if p.reason & REASON_URGENT != 0 => None,
                None => refresh.minutes().map(|m| m as u64 * 60),
            };
            match next_wait {
                Some(wait) => p.next_raise_s = now_s + wait,
                // This raise is the request's last (urgent past the ladder, or `Off` with no
                // cadence): leave one discoverable/readable window, then lapse without beaconing.
                None => {
                    p.final_raise = true;
                    p.next_raise_s = now_s + WEATHER_REQUEST_WINDOW_S;
                }
            }
            return Some(raise);
        }

        // A fresh scheduled request: only while riding, only with a cadence, and only once the
        // interval has elapsed since the last satisfaction. The anchor paces deliberately even
        // when the bundle is unusable (#1221 F3): an accepted upload of a stale/expired bundle is
        // the phone saying "nothing newer exists upstream", and bypassing the anchor for it would
        // re-ask the same upstream seconds later, forever. With no anchor at all (no commit this
        // boot, no bundle to date) the first ride-active poll is due.
        if ride_active {
            if let Some(minutes) = refresh.minutes() {
                let due = match self.anchor_s() {
                    Some(anchor) => now_s as i64 >= anchor + minutes as i64 * 60,
                    None => true,
                };
                if due {
                    let request_id = self.mint_id();
                    let reason = REASON_SCHEDULED | no_bundle_bit | location_bit | hourly_only_bit;
                    self.pending = Some(Pending {
                        request_id,
                        reason,
                        next_raise_s: now_s + RETRY_LADDER_S[0],
                        raises: 1,
                        final_raise: false,
                    });
                    return Some(Raise { request_id, reason });
                }
            }
        }
        None
    }

    /// The absolute second of the next thing this machine will do under the given levels, or `None`
    /// when only an event edge (ride start, urgent, commit, a setting change, a card mount) can
    /// wake it. The caller sleeps until this — never a periodic tick.
    pub fn next_wake_s(&self, refresh: WeatherRefresh, ride_active: bool, store_ready: bool) -> Option<u64> {
        if !store_ready {
            return None;
        }
        if let Some(p) = self.pending {
            return Some(p.next_raise_s);
        }
        if !ride_active {
            return None;
        }
        let minutes = refresh.minutes()?;
        // Pacing is the anchor's alone (never the bundle's freshness) — see `poll`'s scheduled arm.
        match self.anchor_s() {
            Some(anchor) => Some((anchor + minutes as i64 * 60).max(0) as u64),
            None => self.started_s,
        }
    }

    /// Where the interval countdown is anchored: the last commit this boot, else the reboot
    /// reconstruction from the bundle's age (which can lie before boot — signed).
    fn anchor_s(&self) -> Option<i64> {
        self.last_commit_s.map(|s| s as i64).or(self.boot_anchor_s)
    }

    fn mint_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }
}

impl Default for DueScheduler {
    fn default() -> Self {
        Self::new()
    }
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
