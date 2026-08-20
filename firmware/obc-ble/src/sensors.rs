//! Standard cycling-sensor GATT profile codecs — the radio-free half of the BLE-sensors epic
//! (#707). Pure byte→struct parsers for the three profiles a head unit reads (Heart Rate, Cycling
//! Power, Cycling Speed & Cadence) plus Battery Level, and the crank-rev→rpm accumulator that turns
//! a cumulative-count notification stream into an instantaneous cadence.
//!
//! This is exactly `obc-ble`'s charter: `no_std`, no-alloc, no trouble-host / SDC type — the board
//! crate builds its `Uuid`s from the [UUID constants](#constants), subscribes to the
//! characteristics, and feeds the raw notification bytes straight into these parsers, while the
//! host `cargo test`s them against real captures.
//!
//! Every parser is **tolerant**: a short or garbled notification yields `None` rather than a panic
//! — real straps and meters do emit runt frames, so each field is length-checked before it is read.
//!
//! Only the crank-cadence path is consumed in v1 (see epic #707 locked decisions: raw values only,
//! HR + power + cadence). Wheel-revolution data in the CSC frame is parsed into the struct but
//! ignored by consumers — a future wheel-speed feature reuses the codec unchanged.

/// Heart Rate **service** (0x180D).
pub const UUID_HEART_RATE_SERVICE: u16 = 0x180D;
/// Cycling Power **service** (0x1818).
pub const UUID_CYCLING_POWER_SERVICE: u16 = 0x1818;
/// Cycling Speed and Cadence **service** (0x1816).
pub const UUID_CSC_SERVICE: u16 = 0x1816;
/// Battery **service** (0x180F).
pub const UUID_BATTERY_SERVICE: u16 = 0x180F;

/// Heart Rate Measurement **characteristic** (0x2A37) — parsed by [`parse_hr_measurement`].
pub const UUID_HR_MEASUREMENT: u16 = 0x2A37;
/// Cycling Power Measurement **characteristic** (0x2A63) — parsed by [`parse_power_measurement`].
pub const UUID_CYCLING_POWER_MEASUREMENT: u16 = 0x2A63;
/// CSC Measurement **characteristic** (0x2A5B) — parsed by [`parse_csc_measurement`].
pub const UUID_CSC_MEASUREMENT: u16 = 0x2A5B;
/// Battery Level **characteristic** (0x2A19) — parsed by [`parse_battery_level`].
pub const UUID_BATTERY_LEVEL: u16 = 0x2A19;

/// One heart-rate notification: the beats-per-minute reading plus, when the sensor reports it,
/// whether skin contact is currently detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrSample {
    pub bpm: u16,
    /// `Some(true/false)` when the sensor advertises the contact feature; `None` when it doesn't.
    pub contact: Option<bool>,
}

/// A crank-revolution reading: the cumulative count and the timestamp of the last crank event. Both
/// fields wrap at `u16`; [`CrankCadence`] turns a pair of these into an rpm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrankRevs {
    /// Cumulative crank revolutions (wraps at `u16::MAX`).
    pub revs: u16,
    /// Time of the last crank event in 1/1024 s units (wraps at `u16::MAX`).
    pub event_time_1024: u16,
}

/// A wheel-revolution reading from a CSC frame. Parsed for completeness; **unused in v1** — a
/// future wheel-speed feature consumes it (epic #707).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelRevs {
    /// Cumulative wheel revolutions (a `u32` counter on the wire).
    pub revs: u32,
    /// Time of the last wheel event in 1/1024 s units (wraps at `u16::MAX`).
    pub event_time_1024: u16,
}

/// One Cycling Power notification: the mandatory instantaneous power plus optional crank data (the
/// only optional field this crate surfaces — it feeds the cadence quantity when no dedicated cadence
/// sensor is saved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerSample {
    /// Instantaneous power in watts. Signed on the wire (regen/coasting meters can report negative).
    pub watts: i16,
    pub crank: Option<CrankRevs>,
}

/// One CSC notification: wheel and/or crank cumulative data, each present per its flag bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CscSample {
    /// Present iff the wheel-data flag is set. Parsed but unused in v1.
    pub wheel: Option<WheelRevs>,
    pub crank: Option<CrankRevs>,
}

/// Parse a **Heart Rate Measurement** (0x2A37) notification.
///
/// Layout: `flags: u8` at `[0]`, then the bpm value — `u16` LE at `[1..3]` when flag bit 0 is set,
/// else `u8` at `[1]`. Flag bit 2 is the contact-**supported** bit and bit 1 the contact-**status**
/// bit: `contact` is `Some(status)` only when supported. Energy-expended (bit 3) and RR-interval
/// (bit 4) fields follow the bpm value; they carry no data this crate needs, so they are ignored —
/// but the bpm field's own length is checked first, so a frame too short for its declared bpm
/// format yields `None`.
pub fn parse_hr_measurement(data: &[u8]) -> Option<HrSample> {
    let &flags = data.first()?;
    let wide = flags & 0b0000_0001 != 0;
    let bpm = if wide { u16::from_le_bytes([*data.get(1)?, *data.get(2)?]) } else { *data.get(1)? as u16 };
    // Bit 2 = contact supported, bit 1 = contact status. Unsupported → we report nothing.
    let contact = if flags & 0b0000_0100 != 0 { Some(flags & 0b0000_0010 != 0) } else { None };
    Some(HrSample { bpm, contact })
}

/// Parse a **Cycling Power Measurement** (0x2A63) notification.
///
/// Mandatory head: `flags: u16` LE at `[0..2]`, `instantaneous power: i16` LE at `[2..4]`. To reach
/// the crank field we walk the optional fields in spec order, skipping what precedes it:
/// pedal-power-balance (bit 0, +1 B), accumulated-torque (bit 2, +2 B), wheel-rev data (bit 4,
/// +6 B), then crank-rev data (bit 5, `revs: u16` + `event_time: u16`). Anything after crank data
/// is ignored. A buffer too short at any skipped field or the crank field itself → `None` — a frame
/// whose flags declare fields the buffer doesn't hold is garbled, so even its mandatory head isn't
/// trusted (crank flag present or not).
pub fn parse_power_measurement(data: &[u8]) -> Option<PowerSample> {
    let flags = u16::from_le_bytes([*data.first()?, *data.get(1)?]);
    let watts = i16::from_le_bytes([*data.get(2)?, *data.get(3)?]);

    let mut off = 4usize;
    // Walk the optional fields that precede crank data.
    if flags & (1 << 0) != 0 {
        off += 1; // pedal power balance (u8)
    }
    if flags & (1 << 2) != 0 {
        off += 2; // accumulated torque (u16)
    }
    if flags & (1 << 4) != 0 {
        off += 6; // wheel-rev data (u32 revs + u16 event time)
    }
    // The buffer must hold everything the flags declared up to here — a frame truncated inside a
    // skipped field is garbled even when we don't read the crank data after it.
    if data.len() < off {
        return None;
    }

    let crank = if flags & (1 << 5) != 0 {
        let revs = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]);
        let event_time_1024 = u16::from_le_bytes([*data.get(off + 2)?, *data.get(off + 3)?]);
        Some(CrankRevs { revs, event_time_1024 })
    } else {
        None
    };

    Some(PowerSample { watts, crank })
}

/// Parse a **CSC Measurement** (0x2A5B) notification.
///
/// `flags: u8` at `[0]`: bit 0 = wheel data present (`revs: u32` + `event_time: u16`, 6 B), bit 1 =
/// crank data present (`revs: u16` + `event_time: u16`, 4 B), wheel first. V1 consumes crank only;
/// wheel is parsed anyway for a future wheel-speed feature. Short at either field → `None`.
pub fn parse_csc_measurement(data: &[u8]) -> Option<CscSample> {
    let &flags = data.first()?;
    let mut off = 1usize;

    let wheel = if flags & (1 << 0) != 0 {
        let revs = u32::from_le_bytes([*data.get(off)?, *data.get(off + 1)?, *data.get(off + 2)?, *data.get(off + 3)?]);
        let event_time_1024 = u16::from_le_bytes([*data.get(off + 4)?, *data.get(off + 5)?]);
        off += 6;
        Some(WheelRevs { revs, event_time_1024 })
    } else {
        None
    };

    let crank = if flags & (1 << 1) != 0 {
        let revs = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]);
        let event_time_1024 = u16::from_le_bytes([*data.get(off + 2)?, *data.get(off + 3)?]);
        Some(CrankRevs { revs, event_time_1024 })
    } else {
        None
    };

    Some(CscSample { wheel, crank })
}

/// Parse a **Battery Level** (0x2A19) read/notification: a single `u8` percentage, clamped to
/// 0–100. Empty buffer → `None`.
pub fn parse_battery_level(data: &[u8]) -> Option<u8> {
    Some((*data.first()?).min(100))
}

/// Turns a stream of cumulative [`CrankRevs`] readings into an instantaneous cadence in rpm.
///
/// A cadence sensor notifies a *cumulative* crank count plus the timestamp of the last crank event;
/// the rpm is the ratio of the deltas between two notifications:
///
/// `rpm = Δrevs / Δt · 60`, with `Δt = Δevent_time / 1024` seconds.
///
/// Both wire fields wrap at `u16`, so the deltas are computed with [`u16::wrapping_sub`]. The corner
/// cases (all per epic #707):
///
/// - **Coasting** (revs unchanged — the sensor keeps notifying ~1 Hz with a frozen event time) →
///   `Some(0)`.
/// - **Duplicate / garbled** (`Δt == 0` but revs advanced) → `None`, and the baseline is held so the
///   next well-formed frame still computes against a stable reference.
/// - The result is clamped to `u8` (255 rpm is past any human).
///
/// [`reset`](Self::reset) drops the baseline on disconnect so a reconnect doesn't compute a delta
/// across the gap.
///
/// Deliberately **not `Copy`**: a stateful accumulator that copies silently invites updating a
/// copy (a closure capture, say) while the original goes stale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrankCadence {
    last: Option<CrankRevs>,
}

impl CrankCadence {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Feed the next reading. Returns the instantaneous rpm, or `None` when a cadence can't yet be
    /// derived (first sample after construction/reset, or a duplicate-event hold).
    pub fn update(&mut self, r: CrankRevs) -> Option<u8> {
        let Some(prev) = self.last else {
            // No baseline yet — remember this one and wait for the next to form a delta.
            self.last = Some(r);
            return None;
        };

        let d_revs = r.revs.wrapping_sub(prev.revs);
        let d_time = r.event_time_1024.wrapping_sub(prev.event_time_1024);

        if d_revs == 0 {
            // Coasting: no new crank event. Advance the baseline (harmless — fields are unchanged).
            self.last = Some(r);
            return Some(0);
        }
        if d_time == 0 {
            // Revs moved but time didn't: a duplicate or garbled frame. Hold the baseline so the
            // next good frame computes a sane delta rather than dividing by zero.
            return None;
        }

        self.last = Some(r);
        // u64 keeps the numerator (max ~4.0e9) clear of any overflow before the divide + clamp.
        let rpm = (d_revs as u64 * 1024 * 60) / d_time as u64;
        Some(rpm.min(255) as u8)
    }

    /// Forget the baseline (call on disconnect) so the first reading after a reconnect starts a
    /// fresh delta instead of straddling the gap.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

// ============================ Scan-side classification (SE6, #713) ============================
//
// The radio-free half of the board's central scan path (`ble/sensors.rs`): given the raw AD-structure
// bytes of an advertisement, decide whether it is a supported cycling sensor and, if so, which
// quantity it serves. Kept here (not in the board crate) precisely because it is pure byte→enum logic
// — `cargo test` on the host pins it, and the board manager only supplies the bytes from a
// trouble-host scan report and copies the borrowed name into its own `heapless` snapshot.

/// The three sensor quantities the head unit reads (epic #707: HR + power + cadence). Each maps to a
/// standard GATT service and its measurement characteristic — the pair the central discovers and
/// subscribes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    /// Heart Rate (0x180D / 0x2A37).
    HeartRate,
    /// Cycling Power (0x1818 / 0x2A63) — its optional crank data can also feed cadence.
    Power,
    /// Cycling Speed & Cadence (0x1816 / 0x2A5B) — the dedicated cadence sensor.
    Cadence,
}

impl SensorKind {
    /// The primary GATT **service** UUID the manager discovers this sensor by.
    pub const fn service_uuid(self) -> u16 {
        match self {
            SensorKind::HeartRate => UUID_HEART_RATE_SERVICE,
            SensorKind::Power => UUID_CYCLING_POWER_SERVICE,
            SensorKind::Cadence => UUID_CSC_SERVICE,
        }
    }

    /// The measurement **characteristic** UUID the manager subscribes to for notifications.
    pub const fn measurement_uuid(self) -> u16 {
        match self {
            SensorKind::HeartRate => UUID_HR_MEASUREMENT,
            SensorKind::Power => UUID_CYCLING_POWER_MEASUREMENT,
            SensorKind::Cadence => UUID_CSC_MEASUREMENT,
        }
    }
}

/// A supported sensor recognised in a scan advertisement: which quantity, plus its advertised local
/// name (borrowed out of the AD bytes) when present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvMatch<'a> {
    pub kind: SensorKind,
    /// The complete (0x09) or shortened (0x08) local name, UTF-8-validated; `None` when the
    /// advertisement carries neither or the bytes aren't valid UTF-8.
    pub name: Option<&'a str>,
}

/// Priority rank for the classification tie-break: HR > Power > Cadence > (no match). A device that
/// advertises several supported services is reported as the highest-priority one.
const fn kind_rank(k: Option<SensorKind>) -> u8 {
    match k {
        Some(SensorKind::HeartRate) => 3,
        Some(SensorKind::Power) => 2,
        Some(SensorKind::Cadence) => 1,
        None => 0,
    }
}

/// Classify an advertisement's **AD structures** (`[len][type][data…]` repeated) as a supported
/// cycling sensor.
///
/// Walks the AD list once, tolerant of truncation (a runt structure ends the walk rather than
/// panicking), reading the two things a scan list needs:
///
/// - the **16-bit Service UUID** lists (types 0x02 incomplete / 0x03 complete): the first of
///   HR → Power → Cadence found decides [`SensorKind`] (a power meter that also advertises CSC is a
///   power meter, so HR/Power win over Cadence);
/// - the **Local Name** (types 0x09 complete / 0x08 shortened): complete preferred, UTF-8-validated.
///
/// Returns `None` when no supported service UUID appears — i.e. the advertiser is not a sensor we
/// pair with. The name is a borrow into `ad`; the caller copies it into its own fixed buffer.
pub fn classify_advertisement(ad: &[u8]) -> Option<AdvMatch<'_>> {
    let mut kind: Option<SensorKind> = None;
    let mut name: Option<&str> = None;
    let mut name_complete = false;

    let mut i = 0usize;
    while i < ad.len() {
        let len = ad[i] as usize;
        if len == 0 {
            break; // an explicit zero-length field marks the end of the AD data
        }
        // `len` counts the type byte + the payload; a structure that overruns the buffer is a runt.
        let end = i + 1 + len;
        if end > ad.len() {
            break;
        }
        let ad_type = ad[i + 1];
        let payload = &ad[i + 2..end];
        match ad_type {
            // Incomplete / Complete list of 16-bit Service Class UUIDs (LE pairs).
            0x02 | 0x03 => {
                for pair in payload.as_chunks::<2>().0 {
                    let uuid = u16::from_le_bytes([pair[0], pair[1]]);
                    let hit = if uuid == UUID_HEART_RATE_SERVICE {
                        Some(SensorKind::HeartRate)
                    } else if uuid == UUID_CYCLING_POWER_SERVICE {
                        Some(SensorKind::Power)
                    } else if uuid == UUID_CSC_SERVICE {
                        Some(SensorKind::Cadence)
                    } else {
                        None
                    };
                    // Keep the highest-priority match seen (HR > Power > Cadence).
                    if kind_rank(hit) > kind_rank(kind) {
                        kind = hit;
                    }
                }
            }
            // Shortened (0x08) / Complete (0x09) Local Name — prefer the complete form.
            0x08 | 0x09 => {
                let complete = ad_type == 0x09;
                if name.is_none() || (complete && !name_complete) {
                    if let Ok(s) = core::str::from_utf8(payload) {
                        name = Some(s);
                        name_complete = complete;
                    }
                }
            }
            _ => {}
        }
        i = end;
    }

    kind.map(|kind| AdvMatch { kind, name })
}

/// Cadence arbitration (epic #707 locked decision): a **dedicated** cadence sensor owns the cadence
/// quantity; only when none is saved does a power meter's crank data fill it. Returns whether a
/// power-meter crank reading should be dispatched as cadence.
pub const fn power_crank_feeds_cadence(dedicated_cadence_saved: bool) -> bool {
    !dedicated_cadence_saved
}
