//! Recorded-track fixed-record constants.

pub const RECORD_LEN: usize = 20;
pub const FLAG_SEGMENT_START: u16 = 0x0001;
pub const HR_NONE: u8 = 0xFF;
pub const CAD_NONE: u8 = 0xFF;
pub const PWR_NONE: u16 = 0xFFFF;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_width_pins_the_documented_layout() {
        assert_eq!(RECORD_LEN, 4 + 4 + 2 + 2 + 4 + 1 + 1 + 2);
        assert_eq!(FLAG_SEGMENT_START, 1);
    }
}
