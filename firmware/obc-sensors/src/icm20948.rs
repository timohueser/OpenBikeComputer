//! Pure register map + raw→µT scaling for the **magnetometer** of a TDK InvenSense **ICM-20948**
//! 9-axis IMU — the host-testable, chip-specific half of the compass driver (the board crate owns
//! the concrete I²C transactions).
//!
//! ## Only the magnetometer, via I²C bypass
//! The ICM-20948 bundles an accel, gyro and an **AK09916** magnetometer; we use **only the AK09916's
//! three axes** (accel/gyro left asleep — see [`crate::compass`] for why a flat mag-only heading is
//! enough). The AK09916 normally hangs off the ICM's *auxiliary* I²C bus, but
//! [`INT_PIN_CFG_BYPASS_EN`] connects that bus through to the host pins, so it answers directly at
//! [`AK_ADDR`] as if it were standalone — so in bypass the `AK_*` code here **is** standalone-mag
//! code, and swapping the chip leaves [`crate::compass`] untouched.
//!
//! ## Register banks
//! The ICM's registers are paged ([`REG_BANK_SEL`]); everything we touch lives in **bank 0** (the
//! power-on default), so the driver only selects bank 0 defensively and never pages around. The
//! AK09916's registers are *not* banked — it's a separate I²C device.

// ICM-20948 host-side registers (bank 0). Only what bypass bring-up needs: identify, wake, bypass.

/// ICM-20948 I²C address with the `AD0` strap low; the breakout's default is [`ADDR_AD0_HIGH`]. The
/// driver probes both.
pub const ADDR_AD0_LOW: u8 = 0x68;
/// ICM-20948 I²C address with `AD0` high (e.g. the SparkFun breakout's default).
pub const ADDR_AD0_HIGH: u8 = 0x69;

/// Register-bank select (`[5:4]` = bank). We only ever write `0x00` (bank 0) to be defensive after a
/// stray reset; all of [`WHO_AM_I`] / [`PWR_MGMT_1`] / [`INT_PIN_CFG`] live there.
pub const REG_BANK_SEL: u8 = 0x7F;
pub const BANK_0: u8 = 0x00;

/// `WHO_AM_I` (bank 0) and its expected value — read at boot to confirm the part is present and
/// addressed. The driver RTT-logs whatever it reads, so a different revision shows up rather than
/// being silently rejected.
pub const WHO_AM_I: u8 = 0x00;
pub const WHO_AM_I_VAL: u8 = 0xEA;

/// `PWR_MGMT_1` (bank 0). Reset leaves `SLEEP` set; [`PWR_MGMT_1_WAKE`] clears it and auto-selects
/// the best clock. (The accel/gyro stay disabled — we never enable them.)
pub const PWR_MGMT_1: u8 = 0x06;
pub const PWR_MGMT_1_WAKE: u8 = 0x01; // CLKSEL=auto, SLEEP=0

/// `INT_PIN_CFG` (bank 0). [`INT_PIN_CFG_BYPASS_EN`] (bit1) ties the ICM's aux I²C bus to the host
/// pins, exposing the AK09916 at [`AK_ADDR`]. The ICM's internal I²C master is off after reset, which
/// is the other half of what bypass needs, so the driver doesn't touch `USER_CTRL`.
pub const INT_PIN_CFG: u8 = 0x0F;
pub const INT_PIN_CFG_BYPASS_EN: u8 = 0x02;

// AK09916 magnetometer (reachable directly at AK_ADDR once the ICM is in bypass) — by design,
// exactly what a standalone 3-axis magnetometer driver would carry.

/// AK09916 I²C address (fixed).
pub const AK_ADDR: u8 = 0x0C;

/// AK09916 device-ID register (`WIA2`) and its value — the magnetometer's analogue of [`WHO_AM_I`],
/// checked after bypass is enabled to confirm the aux bus came through.
pub const AK_WIA2: u8 = 0x01;
pub const AK_WIA2_VAL: u8 = 0x09;

/// `ST1` status: [`AK_ST1_DRDY`] (bit0) flags a completed measurement.
pub const AK_ST1: u8 = 0x10;
pub const AK_ST1_DRDY: u8 = 0x01;

/// Measurement data start (`HXL`): six bytes `HXL,HXH,HYL,HYH,HZL,HZH`, each axis 16-bit **signed,
/// little-endian**. The driver reads the contiguous block [`AK_HXL`]`..=`[`AK_ST2`] in one burst so
/// the mandatory [`AK_ST2`] read (which releases the measurement) happens in the same transaction.
pub const AK_HXL: u8 = 0x11;

/// `ST2` status — **must be read after the data** to signal the AK09916 the read is done (it latches
/// the next sample). [`AK_ST2_HOFL`] (bit3) flags magnetic overflow (the field exceeded the sensor's
/// range → the sample is invalid and should be dropped).
pub const AK_ST2: u8 = 0x18;
pub const AK_ST2_HOFL: u8 = 0x08;

/// Number of bytes in the `HXL..=ST2` burst the driver reads each measurement (`0x11..=0x18`): 6 data
/// + a temperature/dummy byte (`0x17`) + `ST2`.
pub const AK_DATA_LEN: usize = (AK_ST2 - AK_HXL + 1) as usize;

/// `CNTL2` operation-mode register. [`AK_CNTL2_SINGLE`] triggers **one** measurement then auto-returns
/// to power-down — the lowest-power mode and the exact analogue of the BMP581's forced-per-fix read,
/// so the driver kicks one per GPS fix.
pub const AK_CNTL2: u8 = 0x31;
pub const AK_CNTL2_SINGLE: u8 = 0x01;

/// `CNTL3` control. [`AK_CNTL3_SRST`] (bit0) soft-resets the magnetometer; the driver pulses it at
/// boot before the first measurement.
pub const AK_CNTL3: u8 = 0x32;
pub const AK_CNTL3_SRST: u8 = 0x01;

/// AK09916 sensitivity: a fixed **0.15 µT per LSB** (range ±4912 µT). Absolute scale doesn't affect
/// the heading angle (only the field *direction* matters — see [`crate::compass::heading_deg`]), but
/// the driver scales to real µT anyway so the overflow check and any future tilt-comp see physical units.
pub const UT_PER_LSB: f32 = 0.15;

/// Assemble one 16-bit **signed little-endian** axis from its `(low, high)` data bytes — the
/// AK09916's per-axis encoding.
#[inline]
pub fn raw_axis(low: u8, high: u8) -> i16 {
    i16::from_le_bytes([low, high])
}

/// Convert a raw axis count to microtesla ([`UT_PER_LSB`]).
#[inline]
pub fn raw_to_ut(raw: i16) -> f32 {
    raw as f32 * UT_PER_LSB
}

/// The three magnetometer axes (µT) from an `HXL..=ST2` burst (`data.len()` ≥ 6), in the **AK09916's
/// own axis frame** — `(x, y, z)`. The board crate applies the board-mounting remap + hard-iron
/// offset to land these in the device frame before [`crate::compass::heading_deg`]. Returns `None` if
/// the buffer is too short (a truncated I²C read).
pub fn axes_ut(data: &[u8]) -> Option<(f32, f32, f32)> {
    if data.len() < 6 {
        return None;
    }
    let x = raw_to_ut(raw_axis(data[0], data[1]));
    let y = raw_to_ut(raw_axis(data[2], data[3]));
    let z = raw_to_ut(raw_axis(data[4], data[5]));
    Some((x, y, z))
}

/// Whether an `HXL..=ST2` burst reports a **magnetic overflow** (the [`AK_ST2_HOFL`] bit) — such a
/// sample is saturated and the driver drops it. `ST2` is the last byte of the [`AK_DATA_LEN`] burst;
/// `false` if the buffer is too short to contain it.
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
