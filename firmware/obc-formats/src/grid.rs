//! Allocation-free arithmetic for the shared OBCA and OBCT lattice.

pub const GRID_ORIGIN: i64 = -(1 << 28);
pub const WORLD_SIDE: i64 = 1 << 29;
pub const MIN_CELL_LOG2: u32 = 10;
pub const MAX_CELL_LOG2: u32 = 28;

pub const fn cell_size(log2: u32) -> i64 {
    1 << log2
}

pub const fn axis_cells(log2: u32) -> i64 {
    WORLD_SIDE >> log2
}

pub fn id_width(log2: u32) -> usize {
    let mut value = axis_cells(log2) - 1;
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits.max(4)
}

/// Half-open square in `(min_lon, min_lat, max_lon, max_lat)` order.
pub fn cell_square(log2: u32, i: i64, j: i64) -> (i64, i64, i64, i64) {
    let size = cell_size(log2);
    let min_lat = GRID_ORIGIN + i * size;
    let min_lon = GRID_ORIGIN + j * size;
    (min_lon, min_lat, min_lon + size, min_lat + size)
}

pub fn containing_indices(log2: u32, lat: i64, lon: i64) -> (i64, i64) {
    let size = cell_size(log2);
    ((lat - GRID_ORIGIN).div_euclid(size), (lon - GRID_ORIGIN).div_euclid(size))
}

pub fn on_grid_line(value: i64, log2: u32) -> bool {
    (value - GRID_ORIGIN) & (cell_size(log2) - 1) == 0
}

pub fn on_grid_boundary(lat: i64, lon: i64, log2: u32) -> bool {
    on_grid_line(lat, log2) || on_grid_line(lon, log2)
}

pub fn quad_mid(min: i64, max: i64) -> i64 {
    (min + max).div_euclid(2)
}
