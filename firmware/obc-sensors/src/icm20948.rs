//! Pure register map and raw-to-µT scaling for the magnetometer of a TDK InvenSense ICM-20948
//! 9-axis IMU: the host-testable half of the compass driver. The board crate owns the concrete
//! I²C transactions.
//!
//! Only the magnetometer is used, through I²C bypass. The ICM bundles an accel, a gyro and an
//! AK09916 magnetometer, and the accel and gyro stay asleep. The AK09916 normally hangs off the
//! ICM's auxiliary bus, but [`INT_PIN_CFG_BYPASS_EN`] connects that bus through to the host pins,
//! so it answers directly at [`AK_ADDR`] as if it were standalone: the `AK_*` code here is
//! standalone-mag code, and swapping the chip leaves [`crate::compass`] untouched.
//!
//! The ICM's registers are paged, and everything we touch lives in bank 0, the power-on default,
//! so the driver selects bank 0 only defensively. The AK09916's registers are not banked.

// ICM-20948 host-side registers (bank 0): identify, wake, bypass.

/// ICM-20948 I²C address with the `AD0` strap low. The driver probes both addresses.
pub const ADDR_AD0_LOW: u8 = 0x68;
/// ICM-20948 I²C address with `AD0` high (e.g. the SparkFun breakout's default).
pub const ADDR_AD0_HIGH: u8 = 0x69;

/// Register-bank select. Only `0x00` is ever written, defensively after a stray reset.
pub const REG_BANK_SEL: u8 = 0x7F;
pub const BANK_0: u8 = 0x00;

/// `WHO_AM_I` and its expected value, read at boot to confirm the part is present and addressed.
/// The driver logs whatever it reads, so a different revision shows up rather than being rejected.
pub const WHO_AM_I: u8 = 0x00;
pub const WHO_AM_I_VAL: u8 = 0xEA;

/// `PWR_MGMT_1`. Reset leaves `SLEEP` set; [`PWR_MGMT_1_WAKE`] clears it and auto-selects the
/// clock. The accel and gyro stay disabled.
pub const PWR_MGMT_1: u8 = 0x06;
pub const PWR_MGMT_1_WAKE: u8 = 0x01; // CLKSEL=auto, SLEEP=0

/// `INT_PIN_CFG`. [`INT_PIN_CFG_BYPASS_EN`] ties the ICM's aux I²C bus to the host pins, exposing
/// the AK09916 at [`AK_ADDR`]. The ICM's internal I²C master is off after reset, which is the
/// other half of what bypass needs, so the driver never touches `USER_CTRL`.
pub const INT_PIN_CFG: u8 = 0x0F;
pub const INT_PIN_CFG_BYPASS_EN: u8 = 0x02;

// AK09916 magnetometer, reachable directly at AK_ADDR once the ICM is in bypass.

/// AK09916 I²C address (fixed).
pub const AK_ADDR: u8 = 0x0C;

/// AK09916 device-ID register and its value, checked after bypass is enabled to confirm the aux
/// bus came through.
pub const AK_WIA2: u8 = 0x01;
pub const AK_WIA2_VAL: u8 = 0x09;

/// `ST1` status: [`AK_ST1_DRDY`] (bit0) flags a completed measurement.
pub const AK_ST1: u8 = 0x10;
pub const AK_ST1_DRDY: u8 = 0x01;

/// Measurement data start: six bytes, each axis 16-bit signed little-endian. The driver reads the
/// contiguous block up to [`AK_ST2`] in one burst, so the mandatory `ST2` read happens in the same
/// transaction.
pub const AK_HXL: u8 = 0x11;

/// `ST2` status, which must be read after the data to tell the AK09916 the read is done.
/// [`AK_ST2_HOFL`] flags magnetic overflow, so the sample is invalid and is dropped.
pub const AK_ST2: u8 = 0x18;
pub const AK_ST2_HOFL: u8 = 0x08;

/// Bytes in the `HXL..=ST2` burst: 6 data, a dummy byte and `ST2`.
pub const AK_DATA_LEN: usize = (AK_ST2 - AK_HXL + 1) as usize;

/// `CNTL2` operation mode. [`AK_CNTL2_SINGLE`] triggers one measurement and returns to power-down,
/// the lowest-power mode, so the driver kicks one per GPS fix.
pub const AK_CNTL2: u8 = 0x31;
pub const AK_CNTL2_SINGLE: u8 = 0x01;

/// `CNTL3` control; [`AK_CNTL3_SRST`] soft-resets the magnetometer, pulsed at boot.
pub const AK_CNTL3: u8 = 0x32;
pub const AK_CNTL3_SRST: u8 = 0x01;

/// AK09916 sensitivity: a fixed 0.15 µT per LSB. Absolute scale does not affect the heading angle,
/// but the driver scales to real µT so the overflow check sees physical units.
pub const UT_PER_LSB: f32 = 0.15;

/// Assemble one 16-bit signed little-endian axis from its `(low, high)` data bytes.
#[inline]
pub fn raw_axis(low: u8, high: u8) -> i16 {
    i16::from_le_bytes([low, high])
}

/// Convert a raw axis count to microtesla ([`UT_PER_LSB`]).
#[inline]
pub fn raw_to_ut(raw: i16) -> f32 {
    raw as f32 * UT_PER_LSB
}

/// The three magnetometer axes in µT from an `HXL..=ST2` burst, in the AK09916's own axis frame.
/// The board crate applies the mounting remap and hard-iron offset before
/// [`crate::compass::heading_deg`]. `None` if the buffer is too short.
pub fn axes_ut(data: &[u8]) -> Option<(f32, f32, f32)> {
    if data.len() < 6 {
        return None;
    }
    let x = raw_to_ut(raw_axis(data[0], data[1]));
    let y = raw_to_ut(raw_axis(data[2], data[3]));
    let z = raw_to_ut(raw_axis(data[4], data[5]));
    Some((x, y, z))
}

/// Whether an `HXL..=ST2` burst reports a magnetic overflow, which means the sample is saturated
/// and the driver drops it. `false` if the buffer is too short to contain `ST2`.
pub fn overflowed(data: &[u8]) -> bool {
    data.len() >= AK_DATA_LEN && data[AK_DATA_LEN - 1] & AK_ST2_HOFL != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_signed_le_axis() {
        // 0x1234 little-endian over (low=0x34, high=0x12).
        assert_eq!(raw_axis(0x34, 0x12), 0x1234);
        // Negative two's-complement (0xFFFF = -1).
        assert_eq!(raw_axis(0xFF, 0xFF), -1);
    }

    #[test]
    fn scales_to_microtesla() {
        // +100 LSB → 15 µT at 0.15 µT/LSB.
        assert!((raw_to_ut(100) - 15.0).abs() < 1e-4);
        assert!((raw_to_ut(-100) + 15.0).abs() < 1e-4);
    }

    #[test]
    fn axes_from_burst() {
        // x=+1 (0x0001), y=-1 (0xFFFF), z=+2 (0x0002), then dummy + ST2.
        let burst = [0x01, 0x00, 0xFF, 0xFF, 0x02, 0x00, 0x00, 0x00];
        let (x, y, z) = axes_ut(&burst).unwrap();
        assert!((x - 0.15).abs() < 1e-4);
        assert!((y + 0.15).abs() < 1e-4);
        assert!((z - 0.30).abs() < 1e-4);
    }

    #[test]
    fn axes_rejects_short_buffer() {
        assert!(axes_ut(&[0, 0, 0, 0]).is_none());
    }

    #[test]
    fn detects_overflow_in_st2() {
        let mut burst = [0u8; AK_DATA_LEN];
        assert!(!overflowed(&burst));
        burst[AK_DATA_LEN - 1] = AK_ST2_HOFL;
        assert!(overflowed(&burst));
        // A short buffer can't have overflowed.
        assert!(!overflowed(&[0, 0]));
    }
}
