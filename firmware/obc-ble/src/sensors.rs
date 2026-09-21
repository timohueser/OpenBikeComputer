//! Codecs for the standard cycling-sensor GATT profiles: Heart Rate, Cycling Power, Cycling Speed
//! and Cadence (CSC), and Battery Level. Also the accumulator that makes an rpm from a stream of
//! cumulative crank counts. Every parser is tolerant: a short or garbled notification gives `None`,
//! because real straps and meters do send runt frames.

pub const UUID_HEART_RATE_SERVICE: u16 = 0x180D;
pub const UUID_CYCLING_POWER_SERVICE: u16 = 0x1818;
pub const UUID_CSC_SERVICE: u16 = 0x1816;
pub const UUID_BATTERY_SERVICE: u16 = 0x180F;

pub const UUID_HR_MEASUREMENT: u16 = 0x2A37;
pub const UUID_CYCLING_POWER_MEASUREMENT: u16 = 0x2A63;
pub const UUID_CSC_MEASUREMENT: u16 = 0x2A5B;
pub const UUID_BATTERY_LEVEL: u16 = 0x2A19;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrSample {
    pub bpm: u16,
    /// Skin contact; `Some` only when the sensor advertises the contact feature.
    pub contact: Option<bool>,
}

/// Both fields wrap at `u16`; [`CrankCadence`] turns a pair of these into an rpm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrankRevs {
    pub revs: u16,
    /// Time of the last crank event in 1/1024 s units.
    pub event_time_1024: u16,
}

/// Parsed out of a CSC frame, but no consumer reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelRevs {
    pub revs: u32,
    /// Time of the last wheel event in 1/1024 s units; wraps at `u16::MAX`.
    pub event_time_1024: u16,
}

/// One Cycling Power notification. Crank data is the only optional field this crate surfaces; it
/// feeds the cadence quantity when no dedicated cadence sensor is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerSample {
    /// Instantaneous power in watts; a meter can report a negative value.
    pub watts: i16,
    pub crank: Option<CrankRevs>,
}

/// One CSC notification: wheel and/or crank cumulative data, each present per its flag bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CscSample {
    pub wheel: Option<WheelRevs>,
    pub crank: Option<CrankRevs>,
}

/// Parse a Heart Rate Measurement notification: `flags: u8` at `[0]`, then the bpm — `u16` LE at
/// `[1..3]` when flag bit 0 is set, else `u8` at `[1]`. Flag bit 2 is contact-supported and bit 1
/// contact-status. The energy-expended and RR-interval fields that follow carry nothing this crate
/// needs.
pub fn parse_hr_measurement(data: &[u8]) -> Option<HrSample> {
    let &flags = data.first()?;
    let wide = flags & 0b0000_0001 != 0;
    let bpm = if wide { u16::from_le_bytes([*data.get(1)?, *data.get(2)?]) } else { *data.get(1)? as u16 };
    let contact = if flags & 0b0000_0100 != 0 { Some(flags & 0b0000_0010 != 0) } else { None };
    Some(HrSample { bpm, contact })
}

/// Parse a Cycling Power Measurement notification. Mandatory head: `flags: u16` LE at `[0..2]`,
/// instantaneous power `i16` LE at `[2..4]`. The optional fields follow in spec order:
/// pedal-power-balance (bit 0, 1 B), accumulated-torque (bit 2, 2 B), wheel-rev data (bit 4, 6 B),
/// then crank-rev data (bit 5, `revs: u16` + `event_time: u16`). Anything after the crank data is
/// ignored.
pub fn parse_power_measurement(data: &[u8]) -> Option<PowerSample> {
    let flags = u16::from_le_bytes([*data.first()?, *data.get(1)?]);
    let watts = i16::from_le_bytes([*data.get(2)?, *data.get(3)?]);

    let mut off = 4usize;
    if flags & (1 << 0) != 0 {
        off += 1; // pedal power balance (u8)
    }
    if flags & (1 << 2) != 0 {
        off += 2; // accumulated torque (u16)
    }
    if flags & (1 << 4) != 0 {
        off += 6; // wheel-rev data (u32 revs + u16 event time)
    }
    // A frame truncated inside a skipped field is garbled, even if the crank data is not read.
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

/// Parse a CSC Measurement notification. `flags: u8` at `[0]`: bit 0 = wheel data (`revs: u32` +
/// `event_time: u16`, 6 B), bit 1 = crank data (`revs: u16` + `event_time: u16`, 4 B), wheel first.
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

/// A single `u8` percentage, clamped to 100.
pub fn parse_battery_level(data: &[u8]) -> Option<u8> {
    Some((*data.first()?).min(100))
}

/// Turns a stream of cumulative [`CrankRevs`] readings into an instantaneous cadence:
/// `rpm = Δrevs / Δt · 60`, with `Δt = Δevent_time / 1024` seconds. Both wire fields wrap at `u16`,
/// so the deltas wrap too. Coasting gives `Some(0)`, a duplicate frame gives `None` and holds the
/// baseline, and the result is clamped to `u8`.
///
/// Not `Copy`: a copied accumulator can be updated while the original goes stale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrankCadence {
    last: Option<CrankRevs>,
}

impl CrankCadence {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Returns the rpm, or `None` when no cadence can be derived yet.
    pub fn update(&mut self, r: CrankRevs) -> Option<u8> {
        let Some(prev) = self.last else {
            self.last = Some(r);
            return None;
        };

        let d_revs = r.revs.wrapping_sub(prev.revs);
        let d_time = r.event_time_1024.wrapping_sub(prev.event_time_1024);

        if d_revs == 0 {
            // Coasting: no new crank event.
            self.last = Some(r);
            return Some(0);
        }
        if d_time == 0 {
            // Revs moved but time did not: a duplicate frame. Hold the baseline, do not divide by
            // zero.
            return None;
        }

        self.last = Some(r);
        // u64: the numerator reaches ~4.0e9.
        let rpm = (d_revs as u64 * 1024 * 60) / d_time as u64;
        Some(rpm.min(255) as u8)
    }

    /// Call on disconnect: the first reading after a reconnect must not straddle the gap.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// The sensor quantities the head unit reads. Each maps to a GATT service and its measurement
/// characteristic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    HeartRate,
    /// A power meter's optional crank data can also feed cadence.
    Power,
    /// Cycling Speed and Cadence: the dedicated cadence sensor.
    Cadence,
}

impl SensorKind {
    pub const fn service_uuid(self) -> u16 {
        match self {
            SensorKind::HeartRate => UUID_HEART_RATE_SERVICE,
            SensorKind::Power => UUID_CYCLING_POWER_SERVICE,
            SensorKind::Cadence => UUID_CSC_SERVICE,
        }
    }

    pub const fn measurement_uuid(self) -> u16 {
        match self {
            SensorKind::HeartRate => UUID_HR_MEASUREMENT,
            SensorKind::Power => UUID_CYCLING_POWER_MEASUREMENT,
            SensorKind::Cadence => UUID_CSC_MEASUREMENT,
        }
    }
}

/// A supported sensor recognised in a scan advertisement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvMatch<'a> {
    pub kind: SensorKind,
    /// The complete (0x09) or shortened (0x08) local name, borrowed out of the AD bytes; `None`
    /// when the advertisement carries neither, or the bytes are not UTF-8.
    pub name: Option<&'a str>,
}

/// Tie-break when a device advertises more than one supported service.
const fn kind_rank(k: Option<SensorKind>) -> u8 {
    match k {
        Some(SensorKind::HeartRate) => 3,
        Some(SensorKind::Power) => 2,
        Some(SensorKind::Cadence) => 1,
        None => 0,
    }
}

/// Classify an advertisement's AD structures (`[len][type][data…]` repeated) as a supported cycling
/// sensor. Reads the 16-bit service UUID lists (0x02 incomplete, 0x03 complete) and the local name
/// (0x09 complete, 0x08 shortened; complete preferred). `None` when no supported service UUID
/// appears. The name borrows `ad`; the caller copies it into its own buffer.
pub fn classify_advertisement(ad: &[u8]) -> Option<AdvMatch<'_>> {
    let mut kind: Option<SensorKind> = None;
    let mut name: Option<&str> = None;
    let mut name_complete = false;

    let mut i = 0usize;
    while i < ad.len() {
        let len = ad[i] as usize;
        if len == 0 {
            break; // a zero-length field marks the end of the AD data
        }
        // `len` counts the type byte and the payload; a structure past the buffer end is a runt.
        let end = i + 1 + len;
        if end > ad.len() {
            break;
        }
        let ad_type = ad[i + 1];
        let payload = &ad[i + 2..end];
        match ad_type {
            // Lists of 16-bit Service Class UUIDs, as LE pairs.
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
                    // Keep the highest-priority match: HR, then Power, then Cadence.
                    if kind_rank(hit) > kind_rank(kind) {
                        kind = hit;
                    }
                }
            }
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

/// A dedicated cadence sensor owns the cadence quantity. A power meter's crank data fills in only
/// when no dedicated sensor is saved.
pub const fn power_crank_feeds_cadence(dedicated_cadence_saved: bool) -> bool {
    !dedicated_cadence_saved
}
