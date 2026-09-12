//! Cached full-circle panorama in the display's four native tones.

pub const COLUMNS: usize = 960;
pub const ROWS: usize = 222;
pub const PIXEL_BYTES: usize = COLUMNS * ROWS / 4;

/// Four native tones, packed at two bits per pixel. Column zero points north.
#[derive(Clone)]
pub struct Panorama {
    pixels: [u8; PIXEL_BYTES],
    incomplete: [u8; COLUMNS / 8],
}

impl Default for Panorama {
    fn default() -> Self {
        Self { pixels: [0; PIXEL_BYTES], incomplete: [0; COLUMNS / 8] }
    }
}

impl Panorama {
    pub fn has_incomplete_coverage(&self) -> bool {
        self.incomplete.iter().any(|&byte| byte != 0)
    }

    pub fn incomplete_at_bearing_q4(&self, bearing: u16) -> bool {
        let column = ((usize::from(bearing) * COLUMNS + 720) / 1440) % COLUMNS;
        self.incomplete[column / 8] & (1 << (column % 8)) != 0
    }

    pub(super) fn mark_incomplete(&mut self, column: usize) {
        let column = column % COLUMNS;
        self.incomplete[column / 8] |= 1 << (column % 8);
    }

    /// Nearest panorama sample for a compass bearing expressed in quarter degrees.
    pub fn tone_at_bearing_q4(&self, bearing: u16, row: usize) -> u8 {
        self.tone((usize::from(bearing) * COLUMNS + 720) / 1440, row)
    }

    pub fn tone(&self, column: usize, row: usize) -> u8 {
        let i = (column % COLUMNS) * ROWS + row.min(ROWS - 1);
        (self.pixels[i / 4] >> (i % 4 * 2)) & 3
    }

    pub(super) fn set(&mut self, column: usize, row: usize, tone: u8) {
        let i = column * ROWS + row;
        let shift = i % 4 * 2;
        self.pixels[i / 4] = (self.pixels[i / 4] & !(3 << shift)) | (tone << shift);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_pixels_do_not_corrupt_adjacent_columns() {
        let mut image = std::boxed::Box::new(Panorama::default());
        image.set(0, ROWS - 1, 3);
        image.set(1, 0, 2);
        assert_eq!(image.tone(0, ROWS - 1), 3);
        assert_eq!(image.tone(1, 0), 2);
        assert_eq!(image.tone(1, 1), 0);
        assert_eq!(image.tone(COLUMNS, ROWS - 1), 3);
        assert_eq!(image.tone_at_bearing_q4(1440, ROWS - 1), 3);
        image.set(COLUMNS / 4, 10, 2);
        assert_eq!(image.tone_at_bearing_q4(360, 10), 2);
        assert_eq!(image.tone_at_bearing_q4(359, 10), 0);
        assert_eq!(image.tone_at_bearing_q4(361, 10), 0);
    }
}
