//! OBCG corridor extraction over HTTP Range reads (OBCG §7).
//!
//! The read pattern is the spec's, exactly: header, then the directory pages that arithmetic says
//! cover the corridor, then only the non-dry tiles those pages name. A dry tile costs no bytes.
//! Every fetched page carries its own CRC and every fetched tile carries its own CRC, so a
//! corridor consumer proves the integrity of what it read without ever holding the whole object —
//! which is why this never verifies the *object* CRC and instead pins the manifest's copy of it
//! against the header's.
//!
//! The crop is a **copy of the source lattice**, never a resample: the window is chosen by floor
//! division on the microdegree grid, so every cell that comes out is a provider cell with its own
//! extent. Cells the source does not reach stay [`INTENSITY_NODATA`] — missing data is never dry.

use obc_formats::obcg;
use obc_formats::precip4::INTENSITY_NODATA;

use crate::http::{Http, Request, RANGE_CAP};
use crate::manifest::{Bbox, Frame, SourceClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CropError {
    Http(crate::http::HttpError),
    /// The bytes decoded, but a spec rule failed.
    Format(String),
    /// The corridor and the frame's window do not overlap at all.
    OutsideGrid,
}

impl std::fmt::Display for CropError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CropError::Http(error) => write!(f, "{error}"),
            CropError::Format(why) => write!(f, "format: {why}"),
            CropError::OutsideGrid => write!(f, "corridor does not overlap the frame"),
        }
    }
}

/// The cell window of a frame that answers a corridor, on the frame's own lattice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub col_min: u32,
    pub row_min: u32,
    pub width: u32,
    pub height: u32,
    /// The corridor asked for cells the grid does not have — the crop is short at an edge.
    pub clipped: bool,
}

/// One frame, cropped to the corridor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crop {
    pub valid_at: i64,
    pub source_class: SourceClass,
    /// South-west corner of cell `(0,0)` — restated on the source lattice, so the caller can
    /// rebuild exact geography without re-reading the header.
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub cell_size_m: u16,
    pub width: u32,
    pub height: u32,
    /// Row-major, rows advancing **north** — the same orientation as OBCG and OBCW.
    pub cells: Vec<u8>,
    /// Some in-bounds cell is unavailable, or the crop is short of the corridor.
    pub partial: bool,
}

impl Crop {
    pub fn north_udeg(&self) -> i64 {
        self.south_udeg + i64::from(self.height) * i64::from(self.cell_lat_udeg)
    }

    pub fn east_udeg(&self) -> i64 {
        self.west_udeg + i64::from(self.width) * i64::from(self.cell_lon_udeg)
    }

    pub fn cell(&self, col: u32, row: u32) -> Option<u8> {
        if col >= self.width || row >= self.height {
            return None;
        }
        self.cells.get((row * self.width + col) as usize).copied()
    }
}

fn floor_div(numerator: i64, denominator: i64) -> i64 {
    let quotient = numerator / denominator;
    if numerator % denominator < 0 {
        quotient - 1
    } else {
        quotient
    }
}

/// The cell window covering `corridor` inside a frame's grid, clamped to the grid.
pub fn window(header: &obcg::Header, corridor: &Bbox) -> Option<Window> {
    let south = i64::from(header.south_lat_udeg);
    let west = i64::from(header.west_lon_udeg);
    let lat_stride = i64::from(header.cell_lat_udeg);
    let lon_stride = i64::from(header.cell_lon_udeg);
    if corridor.south_udeg >= header.north_lat_udeg()
        || corridor.north_udeg <= south
        || corridor.west_udeg >= header.east_lon_udeg()
        || corridor.east_udeg <= west
    {
        return None;
    }
    let col_min_raw = floor_div(corridor.west_udeg - west, lon_stride);
    let col_max_raw = floor_div(corridor.east_udeg - west, lon_stride);
    let row_min_raw = floor_div(corridor.south_udeg - south, lat_stride);
    let row_max_raw = floor_div(corridor.north_udeg - south, lat_stride);
    let col_min = col_min_raw.max(0);
    let col_max = col_max_raw.min(i64::from(header.width) - 1);
    let row_min = row_min_raw.max(0);
    let row_max = row_max_raw.min(i64::from(header.height) - 1);
    if col_min > col_max || row_min > row_max {
        return None;
    }
    Some(Window {
        col_min: col_min as u32,
        row_min: row_min as u32,
        width: (col_max - col_min + 1) as u32,
        height: (row_max - row_min + 1) as u32,
        clipped: col_min_raw < 0
            || row_min_raw < 0
            || col_max_raw > i64::from(header.width) - 1
            || row_max_raw > i64::from(header.height) - 1,
    })
}

/// The directory indexes of every tile the window touches, ascending.
pub fn tile_indexes(header: &obcg::Header, window: &Window) -> Vec<u32> {
    let edge = u32::from(header.tile_edge);
    let first_col = window.col_min / edge;
    let last_col = (window.col_min + window.width - 1) / edge;
    let first_row = window.row_min / edge;
    let last_row = (window.row_min + window.height - 1) / edge;
    let mut indexes = Vec::new();
    for tile_row in first_row..=last_row {
        for tile_col in first_col..=last_col {
            if let Some(index) = header.tile_index(tile_col, tile_row) {
                indexes.push(index);
            }
        }
    }
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

/// Merge only ranges that already touch. Coalescing never fetches a byte outside the needed set
/// except the gap *between* two needed ranges, which is what the spec allows and nothing more.
pub fn coalesce(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Fetch and crop one frame. `origin` is the service base (no trailing slash needed).
pub fn crop_frame<H: Http>(http: &mut H, origin: &str, frame: &Frame, corridor: &Bbox) -> Result<Crop, CropError> {
    let url = join(origin, &frame.key);

    // 1. header ------------------------------------------------------------------------------
    let head = read(http, &url, 0, obcg::HEADER_LEN as u64 - 1)?;
    let bytes: [u8; obcg::HEADER_LEN] =
        head.as_slice().try_into().map_err(|_| CropError::Format("short header".into()))?;
    let header = obcg::decode_header(&bytes).map_err(|error| CropError::Format(format!("{error:?}")))?;

    // 2. the manifest is a plan; the header is the truth. Any disagreement — including a frame
    //    re-stamped to look current — refuses the frame before a single cell is trusted.
    if !frame.geometry.agrees_with(&header) {
        return Err(CropError::Format("manifest geometry disagrees with the header".into()));
    }
    if u64::from(header.total_len) != frame.bytes {
        return Err(CropError::Format("manifest byte length disagrees with the header".into()));
    }
    if header.object_crc32 != frame.object_crc32 {
        return Err(CropError::Format("manifest object CRC disagrees with the header".into()));
    }
    if header.valid_at != frame.valid_at {
        return Err(CropError::Format("manifest valid_at disagrees with the header".into()));
    }

    let window = window(&header, corridor).ok_or(CropError::OutsideGrid)?;
    let indexes = tile_indexes(&header, &window);

    // 3. directory pages ----------------------------------------------------------------------
    let page_bytes = u64::from(header.page_bytes());
    let mut pages: Vec<u32> = indexes.iter().map(|index| header.page_of_entry(*index)).collect();
    pages.sort_unstable();
    pages.dedup();
    let page_ranges: Vec<(u64, u64)> = pages
        .iter()
        .map(|page| {
            let offset =
                u64::from(header.page_offset(*page).ok_or_else(|| CropError::Format("page out of range".into()))?);
            Ok((offset, offset + page_bytes - 1))
        })
        .collect::<Result<_, CropError>>()?;
    let mut page_data: std::collections::BTreeMap<u32, Vec<u8>> = std::collections::BTreeMap::new();
    for (start, end) in coalesce(page_ranges) {
        let body = read(http, &url, start, end)?;
        let mut offset = start;
        while offset < end + 1 {
            let page = ((offset - obcg::HEADER_LEN as u64) / page_bytes) as u32;
            let local = (offset - start) as usize;
            let slice = body
                .get(local..local + page_bytes as usize)
                .ok_or_else(|| CropError::Format("short directory page".into()))?;
            obcg::validate_page(&header, slice).map_err(|error| CropError::Format(format!("{error:?}")))?;
            page_data.insert(page, slice.to_vec());
            offset += page_bytes;
        }
    }

    // 4. entries, then only the non-dry payloads ------------------------------------------------
    let mut entries = Vec::with_capacity(indexes.len());
    let mut payload_ranges = Vec::new();
    for index in &indexes {
        let page = header.page_of_entry(*index);
        let within = (*index - page * u32::from(header.entries_per_page)) as usize;
        let page_bytes_ref = page_data.get(&page).ok_or_else(|| CropError::Format("missing directory page".into()))?;
        let entry =
            obcg::decode_entry(page_bytes_ref, within).map_err(|error| CropError::Format(format!("{error:?}")))?;
        if entry.is_dry() {
            // §4.1: an edge tile carries no-data padding, so it can never be the dry sentinel.
            if header.tile_is_partial(*index) {
                return Err(CropError::Format("dry sentinel on a partial edge tile".into()));
            }
        } else {
            let start = u64::from(entry.data_offset);
            let end = start + u64::from(entry.encoded_len) - 1;
            if start < u64::from(header.data_offset)
                || end >= u64::from(header.data_offset) + u64::from(header.data_len)
            {
                return Err(CropError::Format("payload outside the data section".into()));
            }
            payload_ranges.push((start, end));
        }
        entries.push((*index, entry));
    }
    let mut payloads: Vec<(u64, Vec<u8>)> = Vec::new();
    for (start, end) in coalesce(payload_ranges) {
        payloads.push((start, read(http, &url, start, end)?));
    }
    let payload_at = |offset: u64, len: usize| -> Option<&[u8]> {
        payloads.iter().find_map(|(base, body)| {
            let local = offset.checked_sub(*base)? as usize;
            body.get(local..local + len)
        })
    };

    // 5. decode + copy into the crop -------------------------------------------------------------
    let mut cells = vec![INTENSITY_NODATA; (window.width * window.height) as usize];
    let edge = u32::from(header.tile_edge);
    let mut tile = vec![0u8; header.tile_cells()];
    for (index, entry) in entries {
        let payload = if entry.is_dry() {
            &[][..]
        } else {
            payload_at(u64::from(entry.data_offset), usize::from(entry.encoded_len))
                .ok_or_else(|| CropError::Format("missing tile payload".into()))?
        };
        obcg::decode_tile_cells(&header, &entry, payload, &mut tile)
            .map_err(|error| CropError::Format(format!("{error:?}")))?;
        let tile_col = index % header.tile_cols();
        let tile_row = index / header.tile_cols();
        for local_row in 0..edge {
            let row = tile_row * edge + local_row;
            if row < window.row_min || row >= window.row_min + window.height {
                continue;
            }
            for local_col in 0..edge {
                let col = tile_col * edge + local_col;
                if col < window.col_min || col >= window.col_min + window.width {
                    continue;
                }
                // §5: a partial tile still decodes to a full square; cells outside the declared
                // grid are no-data and MUST be clipped. The window is already clamped to the
                // grid, so those cells simply never land here.
                let out = (row - window.row_min) * window.width + (col - window.col_min);
                cells[out as usize] = tile[(local_row * edge + local_col) as usize];
            }
        }
    }

    let partial = window.clipped || cells.contains(&INTENSITY_NODATA);
    Ok(Crop {
        valid_at: header.valid_at,
        source_class: frame.source_class,
        south_udeg: i64::from(header.south_lat_udeg) + i64::from(window.row_min) * i64::from(header.cell_lat_udeg),
        west_udeg: i64::from(header.west_lon_udeg) + i64::from(window.col_min) * i64::from(header.cell_lon_udeg),
        cell_lat_udeg: header.cell_lat_udeg,
        cell_lon_udeg: header.cell_lon_udeg,
        cell_size_m: header.cell_size_m,
        width: window.width,
        height: window.height,
        cells,
        partial,
    })
}

fn read<H: Http>(http: &mut H, url: &str, start: u64, end: u64) -> Result<Vec<u8>, CropError> {
    let wanted = (end - start + 1) as usize;
    let response = http.perform(&Request::range(url, start, end), RANGE_CAP).map_err(CropError::Http)?;
    if !response.is_success() {
        return Err(CropError::Http(crate::http::HttpError::Status { code: response.status, retry_after: None }));
    }
    // A server may lawfully answer a Range request with the whole object. Slicing it ourselves
    // is the only safe reading — treating the head as if it were the middle would be silent
    // corruption that every CRC below would then blame on the producer.
    if response.body.len() == wanted {
        return Ok(response.body);
    }
    let start = start as usize;
    response.body.get(start..start + wanted).map(<[u8]>::to_vec).ok_or_else(|| {
        CropError::Http(crate::http::HttpError::RangeNotHonoured(format!("{} bytes", response.body.len())))
    })
}

pub fn join(origin: &str, key: &str) -> String {
    format!("{}/{}", origin.trim_end_matches('/'), key.trim_start_matches('/'))
}
