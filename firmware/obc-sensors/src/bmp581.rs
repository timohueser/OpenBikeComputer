//! Pure register map and conversions for the Bosch BMP581 barometric altimeter: the host-testable
//! half of the baro driver. The board crate owns the concrete I²C transactions.
//!
//! Per GPS fix the driver writes [`ODR_CONFIG`] with [`PWR_MODE_FORCED`] to trigger one
//! conversion, waits the conversion time, reads the data-ready bit and then the 6 data bytes, and
//! converts. Smoothing is on-chip oversampling rather than a software filter.
//!
//! Only relative height change feeds climb and the dead-band lives downstream, so [`pa_to_m`]
//! hard-codes sea-level `P0`: pressure drift shifts every sample equally and cancels.

/// I²C addresses. The breakout straps `SDO`; the driver probes the default then the alternate.
pub const ADDR_DEFAULT: u8 = 0x47;
pub const ADDR_ALT: u8 = 0x46;

/// `CHIP_ID` register and the expected value for a BMP581, read at boot to confirm the part is
/// present and addressed. The driver logs whatever it reads, so a different revision is visible.
pub const CHIP_ID: u8 = 0x01;
pub const CHIP_ID_BMP581: u8 = 0x50;

/// Interrupt-source enable. The data-ready bit in [`INT_STATUS`] only asserts when its source is
/// enabled here, and the register is `0x00` after reset, so the driver must write
/// [`INT_SRC_DRDY_EN`] or the poll never sees a completed conversion.
pub const INT_SOURCE: u8 = 0x15;
pub const INT_SRC_DRDY_EN: u8 = 0x01; // bit0 drdy_data_reg_en
/// Interrupt and status register; [`STATUS_DRDY`] flags a completed conversion. Cleared on read.
pub const INT_STATUS: u8 = 0x27;
pub const STATUS_DRDY: u8 = 0x01;

/// Temperature data, 24-bit signed, LSB-first. The only data address the driver needs: temperature
/// and pressure are contiguous, so one six-byte burst from here carries both.
pub const TEMP_DATA_XLSB: u8 = 0x1D;

/// Oversampling config. [`OSR_DEFAULT`] enables pressure with ×8 pressure and ×1 temperature
/// oversampling, a good climb-resolution against conversion-time balance for a forced read.
pub const OSR_CONFIG: u8 = 0x36;
pub const OSR_DEFAULT: u8 = 0b0101_1000;

/// Output-data-rate and power-mode config. The driver drives it with [`PWR_MODE_FORCED`] each fix,
/// and [`ODR_DEEP_DIS`] disables deep-standby so the trigger is immediate.
pub const ODR_CONFIG: u8 = 0x37;
pub const PWR_MODE_FORCED: u8 = 0b10; // 0=standby, 1=normal, 2=forced, 3=continuous
pub const ODR_DEEP_DIS: u8 = 1 << 7;
/// The value the driver writes to [`ODR_CONFIG`] to trigger one forced conversion (deep-standby off).
pub const ODR_FORCED_TRIGGER: u8 = ODR_DEEP_DIS | PWR_MODE_FORCED;

/// Sea-level reference pressure, Pa. Hard-coded: only relative change matters.
pub const P0_PA: f32 = 101_325.0;

/// Assemble a 24-bit signed little-endian sample into an `i32`, sign-extending from bit 23: the
/// temperature channel's encoding.
pub fn raw24_signed(xlsb: u8, lsb: u8, msb: u8) -> i32 {
    let raw = (msb as u32) << 16 | (lsb as u32) << 8 | xlsb as u32;
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

/// Barometric altitude in metres from pressure in Pa, `h = 44330·(1 − (P/P0)^0.190284)`. The
/// absolute value is uncalibrated; only the difference between samples is meaningful.
pub fn pa_to_m(pa: f32) -> f32 {
    44_330.0 * (1.0 - libm::powf(pa / P0_PA, 0.190_284))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_24bit_samples() {
        assert_eq!(raw24_unsigned(0xA0, 0x4C, 0x06), 0x06_4C_A0);
        // 0x190000 = 1_638_400 → /65536 = 25.0 °C.
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
        // About 89875 Pa is 1000 m on the standard atmosphere, and the formula must be monotonic.
        let h = pa_to_m(89_875.0);
        assert!((h - 1000.0).abs() < 15.0, "≈1000 m, got {h}");
        assert!(pa_to_m(90_000.0) < pa_to_m(80_000.0), "lower pressure reads higher");
    }

    #[test]
    fn only_relative_change_matters_so_p0_offset_cancels() {
        // Two pressures 100 Pa apart give the same climb delta whatever the absolute anchor,
        // because a constant offset cancels in the difference.
        let d1 = pa_to_m(95_000.0) - pa_to_m(95_100.0);
        let d2 = pa_to_m(94_000.0) - pa_to_m(94_100.0);
        // Not identical, because the curve is nonlinear, but within a few cm over a 100 Pa step.
        assert!((d1 - d2).abs() < 0.2, "d1={d1} d2={d2}");
    }
}
