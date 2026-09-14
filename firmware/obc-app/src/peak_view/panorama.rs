//! Cached full-circle panorama in the display's four native tones.

pub const COLUMNS: usize = 960;
pub const ROWS: usize = 222;
pub const PIXEL_BYTES: usize = COLUMNS * ROWS / 4;
pub(super) const SECTOR_COLUMNS: usize = 15;
pub(super) const SECTORS: usize = COLUMNS / SECTOR_COLUMNS;

/// Four native tones, packed at two bits per pixel. Column zero points north.
#[derive(Clone)]
pub struct Panorama {
    pixels: [u8; PIXEL_BYTES],
    incomplete: [u8; COLUMNS / 8],
    pub(super) finished: u64,
}

impl Default for Panorama {
    fn default() -> Self {
        Self { pixels: [0; PIXEL_BYTES], incomplete: [0; COLUMNS / 8], finished: 0 }
    }
}

impl Panorama {
    pub fn ready_at_bearing_q4(&self, bearing: u16) -> bool {
        let column = column_of(bearing);
        self.finished & (1 << (column / SECTOR_COLUMNS)) != 0
    }

    pub fn view_ready(&self, heading: u16, fov: i32) -> bool {
        view_sectors(heading, fov).all(|sector| self.finished & (1 << sector) != 0)
    }

    /// Completed sectors in the view; changes outside it do not need a redraw.
    pub fn view_progress(&self, heading: u16, fov: i32) -> u8 {
        view_sectors(heading, fov).filter(|sector| self.finished & (1 << sector) != 0).count() as u8
    }

    pub fn has_incomplete_coverage(&self) -> bool {
        self.incomplete.iter().any(|&byte| byte != 0)
    }

    pub fn incomplete_at_bearing_q4(&self, bearing: u16) -> bool {
        let column = column_of(bearing);
        self.incomplete[column / 8] & (1 << (column % 8)) != 0
    }

    pub(super) fn mark_incomplete(&mut self, column: usize) {
        let column = column % COLUMNS;
        self.incomplete[column / 8] |= 1 << (column % 8);
    }

    /// Nearest panorama sample for a compass bearing expressed in quarter degrees.
    pub fn tone_at_bearing_q4(&self, bearing: u16, row: usize) -> u8 {
        self.tone(column_of(bearing), row)
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

pub(super) fn column_of(bearing: u16) -> usize {
    ((usize::from(bearing) * COLUMNS + 720) / 1440) % COLUMNS
}

pub(super) fn view_sectors(heading: u16, fov: i32) -> impl Iterator<Item = usize> {
    let (low, high) = view_sector_bounds(heading, fov);
    (low..=high).map(|sector| sector.rem_euclid(SECTORS as i32) as usize)
}

pub(super) fn view_sector_bounds(heading: u16, fov: i32) -> (i32, i32) {
    // Include neighbour pixels and the catalogue's half-degree search.
    let half = (fov / 2 + 6).min(720);
    let sector = |bearing: i32| (bearing * COLUMNS as i32).div_euclid(1440 * SECTOR_COLUMNS as i32);
    (sector(i32::from(heading) - half), sector(i32::from(heading) + half))
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

    #[test]
    fn pending_columns_are_distinct_from_finished_sky_and_missing_coverage() {
        let mut image = std::boxed::Box::new(Panorama::default());
        image.finished = 1 | (1 << 63);
        image.mark_incomplete(0);
        assert!(image.ready_at_bearing_q4(0));
        assert!(image.ready_at_bearing_q4(1440));
        assert!(image.ready_at_bearing_q4(1439));
        assert!(image.incomplete_at_bearing_q4(0));
        assert!(!image.ready_at_bearing_q4(24));
        assert_eq!(image.tone_at_bearing_q4(24, 0), 0);
        assert!(!image.view_ready(0, 240));
        assert_eq!(image.view_progress(0, 240), 2);
        image.finished = u64::MAX;
        assert!(image.view_ready(0, 240));
    }
}
