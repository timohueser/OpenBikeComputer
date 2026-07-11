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
/// is ignored. A buffer too short at any skipped field or the crank field itself → `None`.
pub fn parse_power_measurement(data: &[u8]) -> Option<PowerSample> {
    let flags = u16::from_le_bytes([*data.get(0)?, *data.get(1)?]);
    let watts = i16::from_le_bytes([*data.get(2)?, *data.get(3)?]);

    let mut off = 4usize;
    // Skip the optional fields that precede crank data, bounds-checking each skip.
    if flags & (1 << 0) != 0 {
        off = off.checked_add(1)?; // pedal power balance (u8)
    }
    if flags & (1 << 2) != 0 {
        off = off.checked_add(2)?; // accumulated torque (u16)
    }
    if flags & (1 << 4) != 0 {
        off = off.checked_add(6)?; // wheel-rev data (u32 revs + u16 event time)
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
