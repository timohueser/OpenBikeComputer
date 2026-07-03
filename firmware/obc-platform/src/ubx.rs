//! Pure UBX protocol decode for the u-blox **SAM-M10Q** GNSS receiver — the host-testable half of
//! the GPS driver (the board crate owns the concrete I²C/DDC transport).
//!
//! ## Why UBX NAV-PVT (not NMEA)
//! One binary, checksummed message carries everything the ride pipeline needs —
//! lat/lon/height/velocity/heading/`fixType`/`numSV`/accuracy/time — as **integer** fields. No
//! ASCII float parsing, no multi-sentence reassembly. The M10 emits it on the I²C (DDC) port once
//! per nav epoch when configured via the VALSET key-value API.
//!
//! ## Framing
//! A UBX frame is `B5 62 | class | id | len_lo len_hi | payload[len] | ck_a ck_b`. The 8-bit
//! Fletcher checksum ([`checksum`]) runs over `class .. payload` (**not** the two sync bytes).
//! [`scan_ubx`] finds the next complete, checksum-valid frame and how many bytes it consumed;
//! [`parse_stream`] returns the **freshest** NAV-PVT in a buffer plus the bytes to drain, leaving
//! any trailing partial frame for the next read.

use obc_app::{DateTime, Fix, GpsTime};

/// UBX sync chars — every frame starts `0xB5 0x62`.
const SYNC1: u8 = 0xB5;
const SYNC2: u8 = 0x62;

/// `UBX-NAV` class and the `NAV-PVT` (position/velocity/time) message id + its fixed payload length.
pub const CLASS_NAV: u8 = 0x01;
pub const ID_NAV_PVT: u8 = 0x07;
/// NAV-PVT payload length. The receiver may append fields in a future protocol revision, so the
/// parser accepts `>=` this and reads by fixed offset; today the M10 emits exactly 92.
pub const NAV_PVT_LEN: usize = 92;

/// NAV-PVT `valid` bitfield: bit0 `validDate`, bit1 `validTime`, bit2 `fullyResolved` (UTC settled,
/// no leap-second ambiguity). All three ⇒ the receiver's UTC date+time is trustworthy — the gate
/// [`NavPvt::utc_time`] applies before it stamps the clock.
pub const VALID_TIME_RESOLVED: u8 = 0x07;

/// `UBX-ACK` class with its ACK / NAK ids — the receiver answers each `CFG-VALSET` with one. See
/// [`ack_status`].
pub const CLASS_ACK: u8 = 0x05;
pub const ID_ACK_ACK: u8 = 0x01;
pub const ID_ACK_NAK: u8 = 0x00;

/// `UBX-CFG` class + the `VALSET` (set configuration value) message id — the M10 dropped the legacy
/// `CFG-MSG`, so all runtime config goes through the key-value VALSET API.
pub const CLASS_CFG: u8 = 0x06;
pub const ID_CFG_VALSET: u8 = 0x8A;

/// `UBX-RXM` class + the `PMREQ` (power-management request) message id — the deep-sleep command the
/// driver issues when tracking stops. See [`pmreq_backup`].
pub const CLASS_RXM: u8 = 0x02;
pub const ID_RXM_PMREQ: u8 = 0x41;

// Little-endian field readers — UBX is little-endian throughout. Each returns 0 if the slice is
// too short (callers gate on length first).
fn le_u16(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_i32(b: &[u8], o: usize) -> i32 {
    le_u32(b, o) as i32
}

/// The UBX 8-bit Fletcher checksum over `data` (which is `class | id | len_lo | len_hi | payload`).
/// `ck_a` accumulates the bytes, `ck_b` accumulates `ck_a` — both mod 256.
pub fn checksum(data: &[u8]) -> (u8, u8) {
    let mut ck_a: u8 = 0;
    let mut ck_b: u8 = 0;
    for &b in data {
        ck_a = ck_a.wrapping_add(b);
        ck_b = ck_b.wrapping_add(ck_a);
    }
    (ck_a, ck_b)
}

/// One framed UBX message: its class/id and a borrow of its payload (checksum already verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UbxFrame<'a> {
    pub class: u8,
    pub id: u8,
    pub payload: &'a [u8],
}

/// Outcome of scanning a byte buffer for the next UBX frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan<'a> {
    /// A complete, checksum-valid frame. Drain `consumed` bytes from the **front** of the buffer —
    /// that count includes any junk skipped before the sync plus the whole frame.
    Frame { frame: UbxFrame<'a>, consumed: usize },
    /// No complete frame yet. Drop `discard` leading bytes (noise before a possible partial sync)
    /// and keep the rest, retrying once more bytes arrive. `discard == buf.len()` means the buffer
    /// held no sync byte at all and can be cleared.
    NeedMore { discard: usize },
}

/// Find the next complete, checksum-valid UBX frame in `buf`.
///
/// Skips leading non-sync noise, validates the length + Fletcher checksum, and on success reports
/// how many bytes to drain. A truncated trailing frame yields [`Scan::NeedMore`] with the leading
/// junk to drop, so a streaming caller keeps only the partial frame's bytes. A **bad checksum** is
/// treated as a false sync: skip that one sync byte and keep scanning, so a corrupt frame can't
/// wedge the stream.
pub fn scan_ubx(buf: &[u8]) -> Scan<'_> {
    let mut i = 0usize;
    while i + 1 < buf.len() {
        if buf[i] != SYNC1 {
            i += 1;
            continue;
        }
        if buf[i + 1] != SYNC2 {
            i += 1;
            continue;
        }
        // Need the 4-byte header (class, id, len) after the two sync bytes.
        if i + 6 > buf.len() {
            return Scan::NeedMore { discard: i };
        }
        let class = buf[i + 2];
        let id = buf[i + 3];
        let len = le_u16(buf, i + 4) as usize;
        let frame_end = i + 6 + len + 2; // payload + 2 checksum bytes
        if frame_end > buf.len() {
            return Scan::NeedMore { discard: i };
        }
        let body = &buf[i + 2..i + 6 + len]; // class..=payload — the checksum input
        let (ck_a, ck_b) = checksum(body);
        if ck_a == buf[i + 6 + len] && ck_b == buf[i + 7 + len] {
            return Scan::Frame {
                frame: UbxFrame { class, id, payload: &buf[i + 6..i + 6 + len] },
                consumed: frame_end,
            };
        }
        // Bad checksum: this sync was noise (or a corrupt frame). Step one byte and re-scan.
        i += 1;
    }
    // No (complete) sync pair found; a lone trailing SYNC1 is kept as a possible partial.
    let discard = if buf.last() == Some(&SYNC1) { buf.len() - 1 } else { buf.len() };
    Scan::NeedMore { discard }
}

/// What a single DDC read yielded: the **freshest** NAV-PVT in the buffer (if any) and the number
/// of leading bytes to drain. Any bytes after `consumed` are a trailing partial frame the caller
/// keeps for the next read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamResult {
    pub nav_pvt: Option<NavPvt>,
    pub consumed: usize,
}

/// Drain every complete UBX frame from `buf`, returning the freshest [`NavPvt`] seen (the one the
/// driver acts on) and how many bytes were consumed. Non-NAV-PVT frames (e.g. a VALSET `ACK-ACK`)
/// are skipped but still consumed. Stops at the first incomplete trailing frame, leaving it for the
/// next read.
pub fn parse_stream(buf: &[u8]) -> StreamResult {
    let mut consumed = 0usize;
    let mut latest = None;
    loop {
        match scan_ubx(&buf[consumed..]) {
            Scan::Frame { frame, consumed: n } => {
                if frame.class == CLASS_NAV && frame.id == ID_NAV_PVT {
                    if let Some(pvt) = parse_nav_pvt(frame.payload) {
                        latest = Some(pvt);
                    }
                }
                consumed += n;
            }
            // A NeedMore with no progress means only a partial/empty tail remains; otherwise drain
            // the junk before the partial frame and keep the tail for the next read.
            Scan::NeedMore { discard } => {
                consumed += discard;
                break;
            }
        }
    }
    StreamResult { nav_pvt: latest, consumed }
}

/// The decoded `UBX-NAV-PVT` fields the ride pipeline needs (a subset of the 92-byte payload, read
/// by fixed offset). Integer units exactly as the receiver reports them; [`to_fix`](NavPvt::to_fix)
/// does the conversion + validity gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavPvt {
    /// GPS time-of-week of the nav epoch, ms.
    pub itow: u32,
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
    /// `valid` bitfield (bit0 validDate, bit1 validTime, bit2 fullyResolved).
    pub valid: u8,
    /// Fix type: 0 none, 1 dead-reckoning, 2 2D, 3 3D, 4 GNSS+DR, 5 time-only.
    pub fix_type: u8,
    /// `flags` bitfield; bit0 is `gnssFixOK` (the fix is usable).
    pub flags: u8,
    /// Satellites used in the nav solution.
    pub num_sv: u8,
    /// Longitude, 1e-7 degrees.
    pub lon: i32,
    /// Latitude, 1e-7 degrees.
    pub lat: i32,
    /// Height above ellipsoid, mm.
    pub height_mm: i32,
    /// Height above mean sea level, mm.
    pub hmsl_mm: i32,
    /// Horizontal accuracy estimate, mm.
    pub hacc_mm: u32,
    /// Vertical accuracy estimate, mm.
    pub vacc_mm: u32,
    /// Ground speed (2D), mm/s.
    pub gspeed_mms: i32,
    /// Heading of motion (2D), 1e-5 degrees.
    pub head_mot: i32,
    /// Position dilution of precision, 0.01 units.
    pub pdop: u16,
}

impl NavPvt {
    /// `flags.gnssFixOK` — the receiver's own "this fix is usable" bit.
    #[inline]
    pub fn gnss_fix_ok(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// The receiver's UTC date+time as a [`GpsTime`] **iff** the `valid` bitfield marks date, time,
    /// **and** full resolution all good — else `None`, so the app never stamps the clock from a
    /// half-resolved epoch. Deliberately **independent of** [`to_fix`](NavPvt::to_fix)'s position
    /// gate: the receiver resolves time before a 3D position, so this can deliver a stamp during
    /// acquisition. A leap-second `60` is clamped to `59`.
    pub fn utc_time(&self) -> Option<GpsTime> {
        if self.valid & VALID_TIME_RESOLVED != VALID_TIME_RESOLVED {
            return None;
        }
        Some(GpsTime {
            utc: DateTime { year: self.year, month: self.month, day: self.day, hour: self.hour, minute: self.min },
            second: self.sec.min(59),
        })
    }

    /// Whether this is a usable position fix: a 3D (or GNSS+DR) solution the receiver flags OK.
    /// This is the **lenient bring-up gate** — `fixType >= 3 && gnssFixOK`. Tighten with
    /// [`passes_quality`](NavPvt::passes_quality) once locks are reliable.
    #[inline]
    pub fn is_valid_fix(&self) -> bool {
        self.fix_type >= 3 && self.gnss_fix_ok()
    }

    /// Optional accuracy gate on top of [`is_valid_fix`](NavPvt::is_valid_fix): horizontal accuracy
    /// ≤ `max_hacc_mm` and pDOP ≤ `max_pdop` (each `None` to skip).
    #[inline]
    pub fn passes_quality(&self, max_hacc_mm: Option<u32>, max_pdop: Option<u16>) -> bool {
        max_hacc_mm.is_none_or(|m| self.hacc_mm <= m) && max_pdop.is_none_or(|m| self.pdop <= m)
    }

    /// Convert to the app's [`Fix`] **iff** this is a valid fix, else `None` (so a cold start /
    /// dropout never teleports the camera). Units: lat/lon 1e-7° → 1e-6 µdeg (rounded); `gSpeed`
    /// mm/s → m/s; `headMot` 1e-5° → deg. **Course gating:** below ~walking pace ([`COURSE_MIN_MMS`])
    /// a real receiver's heading is noise, so `course = None`. **No position smoothing** here: the
    /// motion integrator + route matcher downstream own that (double-filtering adds lag and fights
    /// map-matching).
    pub fn to_fix(&self) -> Option<Fix> {
        if !self.is_valid_fix() {
            return None;
        }
        let course = if self.gspeed_mms >= COURSE_MIN_MMS { Some(self.head_mot as f32 / 1e5) } else { None };
        Some(Fix {
            lat: div_round_i32(self.lat, 10),
            lon: div_round_i32(self.lon, 10),
            course,
            speed_mps: Some(self.gspeed_mms as f32 / 1000.0),
        })
    }
}

/// Ground speed (mm/s) below which [`NavPvt::to_fix`] drops the course to `None`. 0.5 m/s ≈ slow
/// walking pace — under it GPS heading is unreliable.
pub const COURSE_MIN_MMS: i32 = 500;

/// Divide `v` by `d` rounding to nearest (ties away from zero), for the 1e-7° → 1e-6° conversion.
/// Integer-only so it carries no f32 rounding error across the ±180° range.
fn div_round_i32(v: i32, d: i32) -> i32 {
    let half = d / 2;
    if v >= 0 {
        (v + half) / d
    } else {
        (v - half) / d
    }
}

/// Parse a NAV-PVT payload (must be at least [`NAV_PVT_LEN`] bytes) into a [`NavPvt`]. Returns
/// `None` if the slice is too short. Fields are read by fixed offset per the UBX protocol.
pub fn parse_nav_pvt(p: &[u8]) -> Option<NavPvt> {
    if p.len() < NAV_PVT_LEN {
        return None;
    }
    Some(NavPvt {
        itow: le_u32(p, 0),
        year: le_u16(p, 4),
        month: p[6],
        day: p[7],
        hour: p[8],
        min: p[9],
        sec: p[10],
        valid: p[11],
        fix_type: p[20],
        flags: p[21],
        num_sv: p[23],
        lon: le_i32(p, 24),
        lat: le_i32(p, 28),
        height_mm: le_i32(p, 32),
        hmsl_mm: le_i32(p, 36),
        hacc_mm: le_u32(p, 40),
        vacc_mm: le_u32(p, 44),
        gspeed_mms: le_i32(p, 60),
        head_mot: le_i32(p, 64),
        pdop: le_u16(p, 76),
    })
}

/// For a `UBX-ACK` frame answering a config message, return `Some(true)` on ACK-ACK, `Some(false)`
/// on ACK-NAK (payload = the class+id being acknowledged), or `None` if it doesn't match
/// `cls`/`id`. The driver uses this to confirm each VALSET (or RTT-warn a NAK).
pub fn ack_status(frame: &UbxFrame<'_>, cls: u8, id: u8) -> Option<bool> {
    if frame.class != CLASS_ACK || frame.payload.len() < 2 || frame.payload[0] != cls || frame.payload[1] != id {
        return None;
    }
    match frame.id {
        ID_ACK_ACK => Some(true),
        ID_ACK_NAK => Some(false),
        _ => None,
    }
}

// VALSET config-key IDs (u-blox M10 interface description). Each key's top bits encode its storage
// size; one VALSET per key so each can be ACK-tracked individually. NB: confirm these IDs + the
// SAM-M10Q TX-Ready PIO against the M10 interface manual on first bring-up.
/// `CFG-I2COUTPROT-UBX` (L): enable UBX output on the I²C/DDC port.
pub const KEY_I2COUTPROT_UBX: u32 = 0x1072_0001;
/// `CFG-I2COUTPROT-NMEA` (L): NMEA output on the I²C/DDC port — we disable it (UBX only).
pub const KEY_I2COUTPROT_NMEA: u32 = 0x1072_0002;
/// `CFG-MSGOUT-UBX_NAV_PVT_I2C` (U1): NAV-PVT output rate on I²C, in nav epochs (1 = every epoch).
pub const KEY_MSGOUT_NAV_PVT_I2C: u32 = 0x2091_0006;
/// `CFG-RATE-MEAS` (U2): nominal time between GNSS measurements, ms.
pub const KEY_RATE_MEAS: u32 = 0x3021_0001;
/// `CFG-RATE-NAV` (U2): number of measurements per nav solution (1 = a fix per measurement).
pub const KEY_RATE_NAV: u32 = 0x3021_0002;
/// `CFG-TXREADY-ENABLED` (L): assert a module PIO when DDC data is pending.
pub const KEY_TXREADY_ENABLED: u32 = 0x10a2_0001;
/// `CFG-TXREADY-POLARITY` (L): 0 = active-high, 1 = active-low.
pub const KEY_TXREADY_POLARITY: u32 = 0x10a2_0002;
/// `CFG-TXREADY-PIN` (U1): the module PIO number wired to TX-Ready.
pub const KEY_TXREADY_PIN: u32 = 0x20a2_0003;
/// `CFG-TXREADY-THRESHOLD` (U2): bytes-pending threshold / 8 that triggers the PIO.
pub const KEY_TXREADY_THRESHOLD: u32 = 0x30a2_0004;
/// `CFG-TXREADY-INTERFACE` (U1): 0 = I²C, 1 = SPI.
pub const KEY_TXREADY_INTERFACE: u32 = 0x20a2_0005;
/// `CFG-PM-OPERATEMODE` (U1): receiver power mode while tracking — `0` full power, `1` PSMOO
/// (power-save on/off), `2` PSMCT (cyclic tracking). The `power_saver` toggle drives this to `1`
/// while riding. **VERIFY this key + value semantics against the SAM-M10Q manual on first bring-up**
/// — applied best-effort (a wrong id degrades to full power, not a fault).
pub const KEY_PM_OPERATEMODE: u32 = 0x20d0_0001;

/// Frame a UBX message (`B5 62 | class | id | len | payload | ck`) into `out`. Returns the total
/// frame length, or `None` if `out` is too small. The inverse of [`scan_ubx`].
pub fn frame(out: &mut [u8], class: u8, id: u8, payload: &[u8]) -> Option<usize> {
    let total = 8 + payload.len();
    if out.len() < total {
        return None;
    }
    out[0] = SYNC1;
    out[1] = SYNC2;
    out[2] = class;
    out[3] = id;
    let len = payload.len() as u16;
    out[4] = len as u8;
    out[5] = (len >> 8) as u8;
    out[6..6 + payload.len()].copy_from_slice(payload);
    let (ck_a, ck_b) = checksum(&out[2..6 + payload.len()]);
    out[6 + payload.len()] = ck_a;
    out[7 + payload.len()] = ck_b;
    Some(total)
}

/// Build a `CFG-VALSET` frame (RAM layer) setting a single `key` to a 1-byte (`L`/`U1`) value.
/// Payload = 4-byte header + 4-byte key + 1-byte value.
pub fn valset_u8(out: &mut [u8], key: u32, val: u8) -> Option<usize> {
    let mut payload = [0u8; 9];
    valset_header(&mut payload, key);
    payload[8] = val;
    frame(out, CLASS_CFG, ID_CFG_VALSET, &payload)
}

/// Build a `CFG-VALSET` frame (RAM layer) setting a single `key` to a 2-byte (`U2`) value.
pub fn valset_u16(out: &mut [u8], key: u32, val: u16) -> Option<usize> {
    let mut payload = [0u8; 10];
    valset_header(&mut payload, key);
    payload[8..10].copy_from_slice(&val.to_le_bytes());
    frame(out, CLASS_CFG, ID_CFG_VALSET, &payload)
}

/// `RXM-PMREQ` `flags`: request **backup** mode (deep sleep, RTC + RAM retained) and **force** it
/// even with active comms. Woken by activity on the comms port (the driver pokes the DDC).
const PMREQ_FLAG_BACKUP: u32 = 0x02;
const PMREQ_FLAG_FORCE: u32 = 0x04;

/// Build a `UBX-RXM-PMREQ` frame requesting **backup** (deep sleep) for an infinite duration —
/// the M10 retains its RTC + ephemeris on ~microamps and wakes on the next DDC activity (a fast
/// *warm* fix). 16-byte v0 payload: `version(1) | reserved(3) | duration(4 LE, 0 = until woken) |
/// flags(4 LE, backup|force) | wakeupSources(4, 0 = comms activity)`. Returns the frame length
/// written to `out` (24 B), or `None` if `out` is too small.
pub fn pmreq_backup(out: &mut [u8]) -> Option<usize> {
    let mut payload = [0u8; 16];
    payload[0] = 0x00; // version 0
                       // payload[1..4] reserved, payload[4..8] duration = 0 (infinite, until woken)
    let flags = PMREQ_FLAG_BACKUP | PMREQ_FLAG_FORCE;
    payload[8..12].copy_from_slice(&flags.to_le_bytes());
    // payload[12..16] wakeupSources = 0 — any traffic on the (I²C) comms port wakes it.
    frame(out, CLASS_RXM, ID_RXM_PMREQ, &payload)
}

/// Common VALSET payload prefix (first 8 bytes): `version=0 | layers=RAM | reserved(2) | key(4 LE)`.
/// The value bytes follow at offset 8.
fn valset_header(payload: &mut [u8], key: u32) {
    payload[0] = 0x00; // version 0 (no transaction)
    payload[1] = 0x01; // layers: bit0 = RAM
    payload[2] = 0x00;
    payload[3] = 0x00;
    payload[4..8].copy_from_slice(&key.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classic UBX-CFG-PRT poll `B5 62 06 00 00 00 06 18` independently pins the Fletcher
    /// checksum: over `[06 00 00 00]` it is `(0x06, 0x18)`.
    #[test]
    fn checksum_matches_known_vector() {
        assert_eq!(checksum(&[0x06, 0x00, 0x00, 0x00]), (0x06, 0x18));
    }

    /// Build a NAV-PVT payload with chosen fields at their real offsets.
    fn nav_pvt_payload(fix_type: u8, flags: u8, lat: i32, lon: i32, gspeed: i32, head: i32) -> [u8; NAV_PVT_LEN] {
        let mut p = [0u8; NAV_PVT_LEN];
        p[20] = fix_type;
        p[21] = flags;
        p[23] = 9; // numSV
        p[24..28].copy_from_slice(&lon.to_le_bytes());
        p[28..32].copy_from_slice(&lat.to_le_bytes());
        p[40..44].copy_from_slice(&3200u32.to_le_bytes()); // hAcc 3.2 m
        p[60..64].copy_from_slice(&gspeed.to_le_bytes());
        p[64..68].copy_from_slice(&head.to_le_bytes());
        p[76..78].copy_from_slice(&130u16.to_le_bytes()); // pDOP 1.30
        p
    }

    #[test]
    fn parses_valid_3d_fix_and_converts_units() {
        // 3D + gnssFixOK, moving NE at 5 m/s heading 90°. lat/lon in 1e-7°.
        let p = nav_pvt_payload(3, 0x01, 481_229_050, 78_144_380, 5000, 9_000_000);
        let pvt = parse_nav_pvt(&p).unwrap();
        assert!(pvt.is_valid_fix());
        let fix = pvt.to_fix().unwrap();
        // 1e-7° → 1e-6° (÷10, rounded).
        assert_eq!(fix.lat, 48_122_905);
        assert_eq!(fix.lon, 7_814_438);
        assert_eq!(fix.speed_mps, Some(5.0));
        assert_eq!(fix.course, Some(90.0));
    }

    #[test]
    fn rounds_microdegrees_to_nearest() {
        // lat = 15 in 1e-7° → 1.5 in 1e-6°, rounds away from zero to 2; negatives symmetric.
        assert_eq!(div_round_i32(15, 10), 2);
        assert_eq!(div_round_i32(-15, 10), -2);
        assert_eq!(div_round_i32(14, 10), 1);
    }

    #[test]
    fn no_fix_yields_none() {
        // fixType 0 (acquiring) → no Fix, even though gnssFixOK happens to be set.
        let p = nav_pvt_payload(0, 0x01, 1, 2, 0, 0);
        assert_eq!(parse_nav_pvt(&p).unwrap().to_fix(), None);
        // 3D but gnssFixOK clear (receiver says don't trust it) → also None.
        let p = nav_pvt_payload(3, 0x00, 1, 2, 0, 0);
        assert_eq!(parse_nav_pvt(&p).unwrap().to_fix(), None);
    }

    #[test]
    fn stationary_drops_course_but_keeps_speed() {
        // Below COURSE_MIN_MMS the heading is noise → course None; speed still reported.
        let p = nav_pvt_payload(3, 0x01, 1, 2, 100, 12_345_678);
        let fix = parse_nav_pvt(&p).unwrap().to_fix().unwrap();
        assert_eq!(fix.course, None);
        assert_eq!(fix.speed_mps, Some(0.1));
    }

    /// `utc_time` is gated on `validDate | validTime | fullyResolved` and is **independent of the
    /// position fix** (here `fixType = 0`, no lock); it clamps a leap-second `60`.
    #[test]
    fn utc_time_gated_on_resolved_validity_and_independent_of_fix() {
        let mut p = [0u8; NAV_PVT_LEN]; // fixType stays 0 → no usable fix, yet time can be valid
        p[4..6].copy_from_slice(&2026u16.to_le_bytes());
        p[6] = 6; // month
        p[7] = 30; // day
        p[8] = 14; // hour
        p[9] = 37; // min
        p[10] = 56; // sec
        assert!(parse_nav_pvt(&p).unwrap().to_fix().is_none(), "no position fix this epoch");

        p[11] = 0x00; // no valid bits → rejected even though the fields are populated
        assert_eq!(parse_nav_pvt(&p).unwrap().utc_time(), None, "unresolved time is rejected");
        p[11] = 0x03; // validDate | validTime but NOT fullyResolved → still rejected
        assert_eq!(parse_nav_pvt(&p).unwrap().utc_time(), None, "not fully resolved → rejected");

        p[11] = 0x07; // all three → accepted
        let t = parse_nav_pvt(&p).unwrap().utc_time().expect("resolved time → Some");
        assert_eq!((t.utc.year, t.utc.month, t.utc.day), (2026, 6, 30));
        assert_eq!((t.utc.hour, t.utc.minute, t.second), (14, 37, 56), "seconds kept for the back-date");

        p[10] = 60; // a leap second is clamped so the epoch back-date never under-runs a minute
        assert_eq!(parse_nav_pvt(&p).unwrap().utc_time().unwrap().second, 59, "leap second clamped to 59");
    }

    #[test]
    fn quality_gate_is_opt_in() {
        let pvt = parse_nav_pvt(&nav_pvt_payload(3, 0x01, 1, 2, 0, 0)).unwrap();
        assert!(pvt.passes_quality(None, None), "no thresholds → always passes");
        assert!(pvt.passes_quality(Some(5000), Some(200)), "hAcc 3.2m ≤ 5m, pDOP 1.30 ≤ 2.0");
        assert!(!pvt.passes_quality(Some(1000), None), "hAcc 3.2m > 1m rejects");
        assert!(!pvt.passes_quality(None, Some(100)), "pDOP 1.30 > 1.0 rejects");
    }

    #[test]
    fn scan_finds_frame_after_leading_junk() {
        // A NAV-PVT frame preceded by DDC idle bytes (0xFF) the chip emits between messages.
        let p = nav_pvt_payload(3, 0x01, 10, 20, 0, 0);
        let mut buf = [0xFFu8; 3 + 8 + NAV_PVT_LEN];
        let n = frame(&mut buf[3..], CLASS_NAV, ID_NAV_PVT, &p).unwrap();
        let total = 3 + n;
        match scan_ubx(&buf[..total]) {
            Scan::Frame { frame, consumed } => {
                assert_eq!((frame.class, frame.id), (CLASS_NAV, ID_NAV_PVT));
                assert_eq!(consumed, total, "junk + whole frame consumed");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn scan_needs_more_on_truncated_frame() {
        let p = nav_pvt_payload(3, 0x01, 0, 0, 0, 0);
        let mut f = [0u8; 8 + NAV_PVT_LEN];
        let n = frame(&mut f, CLASS_NAV, ID_NAV_PVT, &p).unwrap();
        // Hand scan only the first half of the frame.
        match scan_ubx(&f[..n / 2]) {
            Scan::NeedMore { discard } => assert_eq!(discard, 0, "partial frame starts at 0, keep all"),
            other => panic!("expected NeedMore, got {other:?}"),
        }
    }

    #[test]
    fn bad_checksum_is_skipped_not_wedged() {
        let p = nav_pvt_payload(3, 0x01, 7, 8, 0, 0);
        let mut good = [0u8; 8 + NAV_PVT_LEN];
        let n = frame(&mut good, CLASS_NAV, ID_NAV_PVT, &p).unwrap();
        // A corrupted frame followed by a clean one. parse_stream must skip the bad frame and still
        // return the good NAV-PVT.
        let mut buf = [0u8; 2 * (8 + NAV_PVT_LEN)];
        buf[..n].copy_from_slice(&good[..n]);
        buf[n - 1] ^= 0xFF; // wreck the first frame's checksum
        buf[n..2 * n].copy_from_slice(&good[..n]);
        let res = parse_stream(&buf[..2 * n]);
        assert!(res.nav_pvt.is_some(), "the clean frame after a corrupt one still parses");
    }

    #[test]
    fn parse_stream_returns_freshest_nav_pvt() {
        // Two NAV-PVTs back to back (e.g. a slow drain) → the second (freshest) wins.
        let mut buf = [0u8; 2 * (8 + NAV_PVT_LEN)];
        let mut off = 0;
        for lat in [100i32, 200] {
            let p = nav_pvt_payload(3, 0x01, lat * 10, 0, 0, 0);
            off += frame(&mut buf[off..], CLASS_NAV, ID_NAV_PVT, &p).unwrap();
        }
        let res = parse_stream(&buf[..off]);
        assert_eq!(res.consumed, off);
        assert_eq!(res.nav_pvt.unwrap().to_fix().unwrap().lat, 200);
    }

    #[test]
    fn valset_frame_round_trips_through_scan() {
        let mut out = [0u8; 20];
        let n = valset_u8(&mut out, KEY_MSGOUT_NAV_PVT_I2C, 1).unwrap();
        match scan_ubx(&out[..n]) {
            Scan::Frame { frame, consumed } => {
                assert_eq!((frame.class, frame.id), (CLASS_CFG, ID_CFG_VALSET));
                assert_eq!(consumed, n);
                // payload = version|layers|rsv|rsv|key(4 LE)|val
                assert_eq!(frame.payload[1], 0x01, "RAM layer");
                assert_eq!(&frame.payload[4..8], &KEY_MSGOUT_NAV_PVT_I2C.to_le_bytes());
                assert_eq!(frame.payload[8], 1, "value byte follows the 8-byte header at offset 8");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn pmreq_backup_frames_an_infinite_backup_request() {
        let mut out = [0u8; 24];
        let n = pmreq_backup(&mut out).unwrap();
        match scan_ubx(&out[..n]) {
            Scan::Frame { frame, consumed } => {
                assert_eq!((frame.class, frame.id), (CLASS_RXM, ID_RXM_PMREQ));
                assert_eq!(consumed, n);
                assert_eq!(frame.payload.len(), 16, "v0 PMREQ payload");
                assert_eq!(frame.payload[0], 0, "version 0");
                assert_eq!(&frame.payload[4..8], &[0, 0, 0, 0], "duration 0 = until woken");
                let flags =
                    u32::from_le_bytes([frame.payload[8], frame.payload[9], frame.payload[10], frame.payload[11]]);
                assert_eq!(flags, 0x06, "backup | force");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn ack_status_matches_acked_message() {
        // ACK-ACK whose payload names CFG-VALSET → Some(true); a NAK → Some(false); mismatch → None.
        let ack = UbxFrame { class: CLASS_ACK, id: ID_ACK_ACK, payload: &[CLASS_CFG, ID_CFG_VALSET] };
        assert_eq!(ack_status(&ack, CLASS_CFG, ID_CFG_VALSET), Some(true));
        let nak = UbxFrame { class: CLASS_ACK, id: ID_ACK_NAK, payload: &[CLASS_CFG, ID_CFG_VALSET] };
        assert_eq!(ack_status(&nak, CLASS_CFG, ID_CFG_VALSET), Some(false));
        assert_eq!(ack_status(&ack, CLASS_NAV, ID_NAV_PVT), None);
    }
}
