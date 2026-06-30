//! Pure register map + conversions for the Bosch **BMP581** barometric altimeter (issue #218) —
//! the host-testable half of the baro driver.
//!
//! The board crate owns the concrete I²C transactions; this module is the dependency-light,
//! `no_std`, **pure** logic: the register addresses + bit patterns the driver pokes, and the
//! integer-raw → physical-unit conversions, so the unit-conversion math unit-tests on the host with
//! no hardware (the same split as [`crate::ubx`]).
//!
//! ## How the driver uses it (forced mode)
//! Per GPS fix the driver writes [`ODR_CONFIG`] with [`PWR_MODE_FORCED`] to trigger **one**
//! conversion (lowest power, and coincident with the GPS instant), waits the conversion time, reads
//! the data-ready bit ([`INT_STATUS`] / [`STATUS_DRDY`]) and then the 6 data bytes, and converts:
//! pressure → [`raw_to_pa`] → [`pa_to_m`], temperature → [`raw_to_c`]. Smoothing is **on-chip**
//! (the [`OSR_CONFIG`] oversampling) rather than a software filter; the BMP581's built-in IIR can be
//! layered on later if a per-fix sample proves noisy.
//!
//! ## Why absolute calibration doesn't matter
//! Only *relative* height change feeds climb, and the climb accumulator's dead-band lives downstream
//! (`obc-route/deadband.rs`). So [`pa_to_m`] hard-codes sea-level `P0 = 101325 Pa`: weather drift
//! shifts every sample by the same offset and cancels in the differences.

/// I²C addresses. The breakout straps `SDO`: high → `0x47` (default), low → `0x46`. The driver
/// probes [`ADDR_DEFAULT`] then [`ADDR_ALT`].
pub const ADDR_DEFAULT: u8 = 0x47;
pub const ADDR_ALT: u8 = 0x46;

/// `CHIP_ID` register and the expected value for a BMP581. Read at boot to confirm the part is
/// present + addressed; the driver RTT-logs whatever it reads (so a different revision/value is
/// visible, not silently rejected). Verify against the datasheet on first bring-up.
pub const CHIP_ID: u8 = 0x01;
pub const CHIP_ID_BMP581: u8 = 0x50;

/// Interrupt-source enable register. The `drdy_data_ready` bit in [`INT_STATUS`] only asserts when
/// its source is enabled here (the register is `0x00` after reset), so the driver writes
/// [`INT_SRC_DRDY_EN`] during config or the data-ready poll never sees a completed conversion.
pub const INT_SOURCE: u8 = 0x15;
pub const INT_SRC_DRDY_EN: u8 = 0x01; // bit0 drdy_data_reg_en
/// Interrupt/status register; [`STATUS_DRDY`] (bit0) flags a completed conversion (once
/// [`INT_SOURCE`]'s drdy source is enabled). Cleared on read.
pub const INT_STATUS: u8 = 0x27;
pub const STATUS_DRDY: u8 = 0x01;

/// Temperature data, 24-bit signed, LSB-first across `XLSB|LSB|MSB`. → °C via [`raw_to_c`].
pub const TEMP_DATA_XLSB: u8 = 0x1D;
/// Pressure data, 24-bit unsigned, LSB-first across `XLSB|LSB|MSB`. → Pa via [`raw_to_pa`]. The six
/// data bytes are contiguous `0x1D..=0x22`, so the driver reads them in one burst from `TEMP_DATA_XLSB`.
pub const PRESS_DATA_XLSB: u8 = 0x20;

/// Oversampling config: `osr_t[2:0] | osr_p[5:3] | press_en bit6`. [`OSR_DEFAULT`] enables pressure
/// with ×8 pressure / ×1 temperature oversampling — a good climb-resolution vs. conversion-time
/// balance for a per-fix forced read.
pub const OSR_CONFIG: u8 = 0x36;
// bit6 press_en=1 | bits[5:3] osr_p=0b011 (×8 pressure) | bits[2:0] osr_t=0b000 (×1 temperature)
pub const OSR_DEFAULT: u8 = 0b0101_1000;

/// Output-data-rate + power-mode config: `pwr_mode[1:0]` in the low bits. We drive it with
/// [`PWR_MODE_FORCED`] each fix; [`ODR_DEEP_DIS`] disables deep-standby so the forced trigger is
/// immediate.
pub const ODR_CONFIG: u8 = 0x37;
pub const PWR_MODE_FORCED: u8 = 0b10; // 0=standby, 1=normal, 2=forced, 3=continuous
pub const ODR_DEEP_DIS: u8 = 1 << 7;
/// The value the driver writes to [`ODR_CONFIG`] to trigger one forced conversion (deep-standby off).
pub const ODR_FORCED_TRIGGER: u8 = ODR_DEEP_DIS | PWR_MODE_FORCED;

/// Sea-level reference pressure, Pa. Hard-coded — only *relative* change matters (see module docs).
pub const P0_PA: f32 = 101_325.0;

/// Assemble a 24-bit **signed** little-endian sample (`xlsb`, `lsb`, `msb`) into an `i32`,
/// sign-extending from bit 23 — the temperature channel's encoding.
pub fn raw24_signed(xlsb: u8, lsb: u8, msb: u8) -> i32 {
    let raw = (msb as u32) << 16 | (lsb as u32) << 8 | xlsb as u32;
    // Sign-extend a 24-bit two's-complement value into i32.
    if raw & 0x80_0000 != 0 {
        (raw | 0xFF00_0000) as i32
    } else {
        raw as i32
    }
}

/// Assemble a 24-bit **unsigned** little-endian sample (`xlsb`, `lsb`, `msb`) — the pressure channel.
pub fn raw24_unsigned(xlsb: u8, lsb: u8, msb: u8) -> u32 {
    (msb as u32) << 16 | (lsb as u32) << 8 | xlsb as u32
}

/// Convert a raw 24-bit signed temperature sample to °C (datasheet scale `/ 2^16`).
pub fn raw_to_c(raw: i32) -> f32 {
    raw as f32 / 65_536.0
}

/// Convert a raw 24-bit unsigned pressure sample to Pa (datasheet scale `/ 2^6`).
pub fn raw_to_pa(raw: u32) -> f32 {
    raw as f32 / 64.0
}

/// Barometric altitude (m) from pressure (Pa) via the standard-atmosphere formula
/// `h = 44330·(1 − (P/P0)^0.190284)` with `P0` = [`P0_PA`]. Absolute value is uncalibrated; only the
/// difference between samples (what climb integrates) is meaningful — see the module docs.
pub fn pa_to_m(pa: f32) -> f32 {
    44_330.0 * (1.0 - libm::powf(pa / P0_PA, 0.190_284))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_24bit_samples() {
        // Pressure: 0x064CA0 = 412_320 raw → /64 = 6442.5 Pa (just a byte-assembly check).
        assert_eq!(raw24_unsigned(0xA0, 0x4C, 0x06), 0x06_4C_A0);
        // Temperature positive: 0x190000 = 1_638_400 → /65536 = 25.0 °C.
        assert_eq!(raw24_signed(0x00, 0x00, 0x19), 0x19_0000);
        assert!((raw_to_c(raw24_signed(0x00, 0x00, 0x19)) - 25.0).abs() < 1e-3);
    }

    #[test]
    fn temperature_sign_extends() {
        // 0xFF0000 is a negative 24-bit value (-65536) → -1.0 °C.
        let raw = raw24_signed(0x00, 0x00, 0xFF);
        assert_eq!(raw, -65_536);
        assert!((raw_to_c(raw) + 1.0).abs() < 1e-3);
    }

    #[test]
    fn pressure_converts_to_pa() {
        // 101325 Pa raw = 101325 * 64 = 6_484_800 = 0x62_F3_40 (xlsb, lsb, msb).
        let raw = raw24_unsigned(0x40, 0xF3, 0x62);
        assert_eq!(raw, 6_484_800);
        assert!((raw_to_pa(raw) - 101_325.0).abs() < 0.5);
    }

    #[test]
    fn altitude_is_zero_at_sea_level_pressure() {
        assert!(pa_to_m(P0_PA).abs() < 0.01);
    }

    #[test]
    fn altitude_rises_as_pressure_falls() {
        // ~89875 Pa ≈ 1000 m on the standard atmosphere; the formula should land near it and be
        // monotonic (lower pressure → higher altitude), which is all climb needs.
        let h = pa_to_m(89_875.0);
        assert!((h - 1000.0).abs() < 15.0, "≈1000 m, got {h}");
        assert!(pa_to_m(90_000.0) < pa_to_m(80_000.0), "lower pressure reads higher");
    }

    #[test]
    fn only_relative_change_matters_so_p0_offset_cancels() {
        // Two pressures 100 Pa apart give the same climb delta regardless of the absolute P0 anchor:
        // a constant weather offset shifts both samples equally and cancels in the difference.
        let d1 = pa_to_m(95_000.0) - pa_to_m(95_100.0);
        let d2 = pa_to_m(94_000.0) - pa_to_m(94_100.0);
        // Not identical (the curve is nonlinear) but within a few cm over a 100 Pa step — the
        // dead-band downstream swallows the rest.
        assert!((d1 - d2).abs() < 0.2, "d1={d1} d2={d2}");
    }
}
