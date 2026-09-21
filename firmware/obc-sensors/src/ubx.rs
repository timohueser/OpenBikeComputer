//! Pure UBX protocol decode for the u-blox SAM-M10Q GNSS receiver: the host-testable half of the
//! GPS driver. The board crate owns the concrete I²C/DDC transport.
//!
//! NAV-PVT carries everything the ride pipeline needs as integer fields in one checksummed
//! message, so there is no ASCII float parsing and no multi-sentence reassembly.
//!
//! A UBX frame is `B5 62 | class | id | len_lo len_hi | payload[len] | ck_a ck_b`, and the 8-bit
//! Fletcher checksum runs over `class .. payload`, not the two sync bytes. [`scan_ubx`] finds the
//! next complete, checksum-valid frame; [`parse_stream`] returns the freshest NAV-PVT in a buffer
//! plus the bytes to drain, leaving a trailing partial frame for the next read.

use obc_ports::{DateTime, Fix, GpsTime};

/// UBX sync chars: every frame starts `0xB5 0x62`.
const SYNC1: u8 = 0xB5;
const SYNC2: u8 = 0x62;

/// `UBX-NAV` class and the `NAV-PVT` (position/velocity/time) message id + its fixed payload length.
pub const CLASS_NAV: u8 = 0x01;
pub const ID_NAV_PVT: u8 = 0x07;
/// NAV-PVT payload length. A future protocol revision may append fields, so the parser accepts
/// `>=` this and reads by fixed offset.
pub const NAV_PVT_LEN: usize = 92;

/// NAV-PVT `valid` bitfield: bit0 `validDate`, bit1 `validTime`, bit2 `fullyResolved`. All three
/// mean the receiver's UTC is trustworthy, which is the gate [`NavPvt::utc_time`] applies.
pub const VALID_TIME_RESOLVED: u8 = 0x07;

/// `UBX-ACK` class with its ACK and NAK ids; the receiver answers each `CFG-VALSET` with one.
pub const CLASS_ACK: u8 = 0x05;
pub const ID_ACK_ACK: u8 = 0x01;
pub const ID_ACK_NAK: u8 = 0x00;

/// `UBX-CFG` class and the `VALSET` message id. The M10 dropped `CFG-MSG`, so all runtime config
/// goes through the key-value VALSET API.
pub const CLASS_CFG: u8 = 0x06;
pub const ID_CFG_VALSET: u8 = 0x8A;

/// Controlled GNSS start/stop command. See [`cfg_gnss_running`].
pub const ID_CFG_RST: u8 = 0x04;

// Little-endian field readers. Each returns 0 if the slice is too short; callers gate on length.
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

/// The UBX 8-bit Fletcher checksum over `class | id | len_lo | len_hi | payload`: `ck_a`
/// accumulates the bytes and `ck_b` accumulates `ck_a`, both mod 256.
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
    /// A complete, checksum-valid frame. Drain `consumed` bytes from the front: that count
    /// includes any junk skipped before the sync.
    Frame { frame: UbxFrame<'a>, consumed: usize },
    /// No complete frame yet. Drop `discard` leading bytes and keep the rest.
    /// `discard == buf.len()` means the buffer held no sync byte at all.
    NeedMore { discard: usize },
}

/// Find the next complete, checksum-valid UBX frame in `buf`.
///
/// A truncated trailing frame yields [`Scan::NeedMore`] with the leading junk to drop, so a
/// streaming caller keeps only the partial frame. A bad checksum is treated as a false sync: skip
/// that one sync byte and keep scanning, so a corrupt frame cannot wedge the stream.
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
        // Bad checksum: this sync was noise. Step one byte and re-scan.
        i += 1;
    }
    // No complete sync pair found; a lone trailing SYNC1 is kept as a possible partial.
    let discard = if buf.last() == Some(&SYNC1) { buf.len() - 1 } else { buf.len() };
    Scan::NeedMore { discard }
}

/// What a single DDC read yielded: the freshest NAV-PVT in the buffer and the number of leading
/// bytes to drain. Bytes after `consumed` are a trailing partial frame the caller keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamResult {
    pub nav_pvt: Option<NavPvt>,
    pub consumed: usize,
}

/// Drain every complete UBX frame from `buf`, returning the freshest [`NavPvt`] and how many
/// bytes were consumed. Other frames are skipped but still consumed. Stops at the first
/// incomplete trailing frame.
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
            // A NeedMore with no progress means only a partial tail remains; otherwise drain the
            // junk before the partial frame and keep the tail.
            Scan::NeedMore { discard } => {
                consumed += discard;
                break;
            }
        }
    }
    StreamResult { nav_pvt: latest, consumed }
}

/// The decoded `UBX-NAV-PVT` fields the ride pipeline needs, read by fixed offset, in the
/// receiver's own integer units. [`to_fix`](NavPvt::to_fix) converts and gates them.
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
    /// `flags.gnssFixOK`: the receiver's own "this fix is usable" bit.
    #[inline]
    pub fn gnss_fix_ok(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// The receiver's UTC as a [`GpsTime`], only if `valid` marks date, time and full resolution
    /// all good, so the app never stamps the clock from a half-resolved epoch. Independent of
    /// [`to_fix`](NavPvt::to_fix)'s position gate, because the receiver resolves time before a 3D
    /// position. A leap-second `60` is clamped to `59`.
    pub fn utc_time(&self) -> Option<GpsTime> {
        if self.valid & VALID_TIME_RESOLVED != VALID_TIME_RESOLVED {
            return None;
        }
        Some(GpsTime {
            utc: DateTime { year: self.year, month: self.month, day: self.day, hour: self.hour, minute: self.min },
            second: self.sec.min(59),
        })
    }

    /// Whether this is a usable position fix: `fixType >= 3 && gnssFixOK`. A lenient bring-up
    /// gate; tighten with [`passes_quality`](NavPvt::passes_quality) once locks are reliable.
    #[inline]
    pub fn is_valid_fix(&self) -> bool {
        self.fix_type >= 3 && self.gnss_fix_ok()
    }

    /// Optional accuracy gate on top of [`is_valid_fix`](NavPvt::is_valid_fix): horizontal
    /// accuracy and pDOP, each `None` to skip.
    #[inline]
    pub fn passes_quality(&self, max_hacc_mm: Option<u32>, max_pdop: Option<u16>) -> bool {
        max_hacc_mm.is_none_or(|m| self.hacc_mm <= m) && max_pdop.is_none_or(|m| self.pdop <= m)
    }

    /// Convert to the app's [`Fix`] only for a valid fix, so a cold start never teleports the
    /// camera. Below about walking pace ([`COURSE_MIN_MMS`]) a receiver's heading is noise, so
    /// `course` is `None`. No position smoothing here: the motion integrator and the route
    /// matcher downstream own that, and double-filtering adds lag.
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

/// Ground speed below which [`NavPvt::to_fix`] drops the course. 0.5 m/s is slow walking pace,
/// under which GPS heading is unreliable.
pub const COURSE_MIN_MMS: i32 = 500;

/// Divide `v` by `d` rounding to nearest, ties away from zero. Integer-only, so it carries no f32
/// rounding error across the ±180° range.
fn div_round_i32(v: i32, d: i32) -> i32 {
    let half = d / 2;
    if v >= 0 {
        (v + half) / d
    } else {
        (v - half) / d
    }
}

/// Parse a NAV-PVT payload into a [`NavPvt`], or `None` if the slice is too short.
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

/// For a `UBX-ACK` frame, `Some(true)` on ACK-ACK, `Some(false)` on ACK-NAK, or `None` if it does
/// not match `cls` and `id`. The driver confirms each VALSET with it.
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

// VALSET config-key IDs. Each key's top bits encode its storage size, and there is one VALSET per
// key so each can be ACK-tracked individually.
/// `CFG-I2COUTPROT-UBX` (L): enable UBX output on the I²C/DDC port.
pub const KEY_I2COUTPROT_UBX: u32 = 0x1072_0001;
/// `CFG-I2COUTPROT-NMEA` (L): NMEA output on the I²C/DDC port, which we disable.
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
/// `CFG-PM-OPERATEMODE` (U1): receiver power mode while tracking. `0` full power, `1` PSMOO,
/// `2` PSMCT. The `power_saver` toggle drives this to `1` while riding. Applied best-effort: a
/// wrong id degrades to full power rather than faulting.
pub const KEY_PM_OPERATEMODE: u32 = 0x20d0_0001;

/// Frame a UBX message into `out`, returning the total frame length or `None` if `out` is too
/// small. The inverse of [`scan_ubx`].
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

/// Build a `CFG-VALSET` frame (RAM layer) setting a single `key` to a 1-byte value.
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

/// Start or stop GNSS tasks without clearing navigation data or configuration. The receiver does
/// not acknowledge this command. Returns 12 bytes, or `None` if `out` is too small.
pub fn cfg_gnss_running(out: &mut [u8], running: bool) -> Option<usize> {
    let mode = if running { 0x09 } else { 0x08 };
    frame(out, CLASS_CFG, ID_CFG_RST, &[0, 0, mode, 0])
}

/// Common VALSET payload prefix: `version=0 | layers=RAM | reserved(2) | key(4 LE)`, with the
/// value bytes at offset 8.
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

    /// The classic UBX-CFG-PRT poll independently pins the Fletcher checksum: over
    /// `[06 00 00 00]` it is `(0x06, 0x18)`.
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

    /// `utc_time` is gated on `validDate | validTime | fullyResolved`, is independent of the
    /// position fix, and clamps a leap-second `60`.
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
        // A NAV-PVT frame behind the DDC idle bytes the chip emits between messages.
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
        // A corrupted frame followed by a clean one: the bad frame is skipped and the good
        // NAV-PVT still comes back.
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
    fn gnss_control_preserves_navigation_data_and_uses_controlled_modes() {
        for (running, expected) in [
            (false, [0xb5, 0x62, 0x06, 0x04, 0x04, 0, 0, 0, 0x08, 0, 0x16, 0x74]),
            (true, [0xb5, 0x62, 0x06, 0x04, 0x04, 0, 0, 0, 0x09, 0, 0x17, 0x76]),
        ] {
            let mut out = [0u8; 12];
            assert_eq!(cfg_gnss_running(&mut out, running), Some(expected.len()));
            assert_eq!(out, expected);
            assert_eq!(cfg_gnss_running(&mut out[..11], running), None);
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
