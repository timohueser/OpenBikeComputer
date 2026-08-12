//! The corridor: the window a fetch asks about, and OBCG extraction over HTTP Range reads (§7).
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
use crate::manifest_v2::{Bbox, ShardGeometry};

/// The two-hour question the rain map answers. A property of the **dataset's** timeline, not of any
/// product: nothing selects on it, it only bounds which frames are worth fetching.
pub const HORIZON_S: i64 = 2 * 3600;
/// How old an observation frame may be and still be worth fetching. Beyond this a "current" frame
/// would be a lie told with a true timestamp.
pub const MAX_OBSERVATION_AGE_S: i64 = 6 * 3600;
/// A manifest stamped this far in the future means the *local* clock is wrong. Reported, never
/// compensated: silently shifting time is how stale rain becomes a dry claim.
pub const CLOCK_SKEW_TOLERANCE_S: i64 = 15 * 60;

/// One degree of latitude, in metres — the corridor arithmetic's only constant.
pub const METRES_PER_DEGREE_LAT: f64 = 111_320.0;

/// The corridor radius, WXR5 #1244: a plain **90 km disc** around the rider.
///
/// Large enough that half an hour of riding does not move the rider out of it, so a corridor is a
/// question about a place rather than about a heading. There is no projection, no bearing and no
/// speed any more: under one uniform lattice a bigger window costs bytes and nothing else, where a
/// directed cone used to change *which product answered* — the tier ladder's real reason for
/// existing, and the thing epic #1248 deleted from the clients (#1244) and then from the producer
/// and the spec (#1246).
pub const CORRIDOR_RADIUS_M: f64 = 90_000.0;

/// The window one fetch asks about, in integer microdegrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corridor {
    pub bounds: Bbox,
    /// The rider's own position. Two things need it and neither is the bbox: a shrunken bundle
    /// window re-centres on the rider rather than on the midpoint of a corridor they are leaving,
    /// and the east-west resample pitch is a function of their latitude.
    pub lat_udeg: i32,
    pub lon_udeg: i32,
    /// The disc ran off the edge of the coordinate system and was cut — at the antimeridian, at a
    /// pole, or both. Evidence for the panel; the honest consequence (a corridor that reaches into
    /// the uncovered polar band) is reported by [`PlanOutcome`](crate::manifest_v2::PlanOutcome),
    /// not inferred from this flag.
    pub clamped: bool,
}

impl Corridor {
    /// The [`CORRIDOR_RADIUS_M`] disc around a position — the only corridor there is.
    pub fn for_rider(lat_udeg: i32, lon_udeg: i32) -> Self {
        Self::around(lat_udeg, lon_udeg, CORRIDOR_RADIUS_M)
    }

    /// A disc of `radius_m` around `(lat_udeg, lon_udeg)`, clamped to the coordinate system.
    ///
    /// **The two clamps are the owed ones (#1244).** Longitude degrees shrink with latitude, so the
    /// east/west growth divides by `cos(lat)` and a disc near either edge of the map would run off
    /// it:
    ///
    /// - **Antimeridian.** `OBCG_Spec.md` §1 and `OBCW_Spec.md` §1 both forbid a v1 grid crossing
    ///   ±180°, so the *bundle* cannot state a wrapped window whatever the manifest reader can read.
    ///   The disc is therefore cut at the date line and the sliver beyond it reads as not covered,
    ///   which is honest. (Wrapped windows — `west > east` — remain fully supported one layer up in
    ///   [`Grid::shards_for`](crate::manifest_v2::Grid::shards_for), where the shared fixture pins
    ///   them in both languages; this is the client declining to *ask* one, not the reader losing
    ///   the ability to answer.)
    /// - **Poles.** Above ~85° the disc's longitudinal extent exceeds the whole map, so the
    ///   `cos` floor bounds it and the latitude clamps at ±90°. A corridor up there lands outside
    ///   the lattice's `covered_rows` and the plan answers `Uncovered` — the honest sentence —
    ///   rather than the client emitting a window no format can express.
    ///
    /// Being a little generous costs one more tile read; being short would drop rain the rider is
    /// about to ride into.
    pub fn around(lat_udeg: i32, lon_udeg: i32, radius_m: f64) -> Self {
        let lat = i64::from(lat_udeg);
        let lon = i64::from(lon_udeg);
        let lat_span = (radius_m / METRES_PER_DEGREE_LAT * 1e6).ceil() as i64;
        let cos = (f64::from(lat_udeg) / 1e6).to_radians().cos().max(0.05);
        let lon_span = (radius_m / (METRES_PER_DEGREE_LAT * cos) * 1e6).ceil() as i64;
        let bounds = Bbox {
            south_udeg: (lat - lat_span).max(-90_000_000),
            west_udeg: (lon - lon_span).max(-180_000_000),
            north_udeg: (lat + lat_span).min(90_000_000),
            east_udeg: (lon + lon_span).min(180_000_000),
        };
        let clamped = lat - lat_span < -90_000_000
            || lat + lat_span > 90_000_000
            || lon - lon_span < -180_000_000
            || lon + lon_span > 180_000_000;
        Self { bounds, lat_udeg, lon_udeg, clamped }
    }
}

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

/// One shard of one frame, cropped to the corridor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crop {
    pub valid_at: i64,
    /// The manifest's per-**shard** `observed` flag: this patch of the mosaic came from a radar,
    /// not from model fill. Per shard because that is where it is true — one frame is radar over
    /// Germany and model over the Atlantic at the same instant.
    pub observed: bool,
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
    /// Some in-bounds cell is unavailable, or the crop is short of the corridor — **per shard**.
    ///
    /// Evidence about this object, and deliberately *not* the frame's answer: a crop is short
    /// whenever the corridor reaches past its own shard's edge, which is the normal case the moment
    /// a corridor straddles a seam and its neighbour supplies exactly the missing cells. The frame's
    /// partial-coverage flag is computed over the assembled frame in `bundle::rain_frame`, where a
    /// cell no shard and no dry rectangle reached is the thing that is actually true.
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
    window_on_lattice(
        i64::from(header.south_lat_udeg),
        i64::from(header.west_lon_udeg),
        i64::from(header.cell_lat_udeg),
        i64::from(header.cell_lon_udeg),
        header.width,
        header.height,
        corridor,
    )
}

/// The same window from the **derived shard geometry**, before a byte is fetched — what lets a
/// cache lookup be keyed on the exact crop a shard would produce without paying for its header
/// first. The header is still checked against that geometry before any cell is trusted.
pub fn window_of(geometry: &ShardGeometry, corridor: &Bbox) -> Option<Window> {
    window_on_lattice(
        i64::from(geometry.south_udeg),
        i64::from(geometry.west_udeg),
        i64::from(geometry.cell_udeg),
        i64::from(geometry.cell_udeg),
        geometry.width,
        geometry.height,
        corridor,
    )
}

fn window_on_lattice(
    south: i64,
    west: i64,
    lat_stride: i64,
    lon_stride: i64,
    grid_width: u32,
    grid_height: u32,
    corridor: &Bbox,
) -> Option<Window> {
    let north = south + i64::from(grid_height) * lat_stride;
    let east = west + i64::from(grid_width) * lon_stride;
    if corridor.south_udeg >= north
        || corridor.north_udeg <= south
        || corridor.west_udeg >= east
        || corridor.east_udeg <= west
    {
        return None;
    }
    let col_min_raw = floor_div(corridor.west_udeg - west, lon_stride);
    let col_max_raw = floor_div(corridor.east_udeg - west, lon_stride);
    let row_min_raw = floor_div(corridor.south_udeg - south, lat_stride);
    let row_max_raw = floor_div(corridor.north_udeg - south, lat_stride);
    let col_min = col_min_raw.max(0);
    let col_max = col_max_raw.min(i64::from(grid_width) - 1);
    let row_min = row_min_raw.max(0);
    let row_max = row_max_raw.min(i64::from(grid_height) - 1);
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
            || col_max_raw > i64::from(grid_width) - 1
            || row_max_raw > i64::from(grid_height) - 1,
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

/// One object the plan says exists, with everything needed to address and verify it.
///
/// The v1 shape of this was a `manifest::Frame` carrying its own geometry; under v2 the geometry is
/// derived from the stated lattice and the identity is `(offset_min, shard)`, so what a reader
/// needs is exactly this: where the bytes are, how many there should be, and what they must decode
/// to agree with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardRead {
    pub key: String,
    pub geometry: ShardGeometry,
    pub bytes: u64,
    pub object_crc32: u32,
    pub valid_at: i64,
    pub observed: bool,
}

/// Fetch and crop one shard. `origin` is the service base (no trailing slash needed).
pub fn crop_frame<H: Http>(http: &mut H, origin: &str, frame: &ShardRead, corridor: &Bbox) -> Result<Crop, CropError> {
    let url = join(origin, &frame.key);

    // 1. header ------------------------------------------------------------------------------
    let head = read(http, &url, 0, obcg::HEADER_LEN as u64 - 1)?;
    let bytes: [u8; obcg::HEADER_LEN] =
        head.as_slice().try_into().map_err(|_| CropError::Format("short header".into()))?;
    let header = obcg::decode_header(&bytes).map_err(|error| CropError::Format(format!("{error:?}")))?;

    // 2. the manifest is a plan; the header is the truth. Any disagreement — including a frame
    //    re-stamped to look current — refuses the frame before a single cell is trusted.
    if !frame.geometry.agrees_with(&header) {
        return Err(CropError::Format("the lattice disagrees with the header".into()));
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
        observed: frame.observed,
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

/// One Range read, with the two statuses a corridor read may act on and nothing else.
///
/// `206` is the answer we asked for: its `Content-Range` must name the bytes we asked for and its
/// body must be exactly that long — an over-long "partial" answer is a server contradicting
/// itself, never something to slice. `200` means the server ignored `Range` and streamed the whole
/// object, which is lawful, so we slice it ourselves rather than read its head as the middle.
/// **Every other 2xx is a refusal**: a `204` or a `203` to a Range request describes something
/// other than the bytes this reader is about to CRC, and guessing which is how a transport lie
/// gets blamed on the producer.
fn read<H: Http>(http: &mut H, url: &str, start: u64, end: u64) -> Result<Vec<u8>, CropError> {
    let wanted = (end - start + 1) as usize;
    let response = http.perform(&Request::range(url, start, end), RANGE_CAP).map_err(CropError::Http)?;
    let not_honoured = |why: String| CropError::Http(crate::http::HttpError::RangeNotHonoured(why));
    match response.status {
        206 => {
            if let Some((first, last)) = response.content_range_bytes() {
                if (first, last) != (start, end) {
                    return Err(not_honoured(format!("Content-Range bytes {first}-{last}, asked {start}-{end}")));
                }
            }
            if response.body.len() != wanted {
                return Err(not_honoured(format!("{} bytes for a {wanted}-byte range", response.body.len())));
            }
            Ok(response.body)
        }
        200 => {
            let start = start as usize;
            response
                .body
                .get(start..start + wanted)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| not_honoured(format!("whole-object answer of {} bytes", response.body.len())))
        }
        code => Err(CropError::Http(crate::http::HttpError::Status { code, retry_after: response.retry_after })),
    }
}

pub fn join(origin: &str, key: &str) -> String {
    format!("{}/{}", origin.trim_end_matches('/'), key.trim_start_matches('/'))
}

// ── the frame cache ────────────────────────────────────────────────────────────────────────

/// One cropped shard's identity: the immutable object key plus the exact cell window taken out of
/// it. Both halves matter. The key is immutable by the publishing contract, which is why a hit is
/// **never revalidated** — the bytes behind `wx/v2/20260810T1430Z/f15/s3-2.obcg` cannot change,
/// so a conditional request against them would be pure latency. The window makes an entry answer
/// only the question it was stored for: a wider corridor is a miss, not a wrong answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub object_key: String,
    pub col_min: u32,
    pub row_min: u32,
    pub width: u32,
    pub height: u32,
}

impl FrameKey {
    pub fn new(object_key: &str, window: &Window) -> Self {
        Self {
            object_key: object_key.to_string(),
            col_min: window.col_min,
            row_min: window.row_min,
            width: window.width,
            height: window.height,
        }
    }
}

/// A bounded, process-lifetime cache of cropped frames, FIFO by insertion — the phone's
/// `InMemoryWeatherFrameCache`, same capacity, same key.
///
/// This is what makes a 30-minute cadence cheap: at a 15-minute frame stride, seven or eight of
/// DWD's nine frames are the *same immutable objects* the previous fetch already cropped, and
/// re-reading them would be bytes spent to learn what the client already knows.
///
/// The phone also has a file cache that survives suspension between two BLE connections; the
/// simulator deliberately has no on-disk half — a `--weather live` process is one session, and a
/// disk cache would add a second place a stale crop could come from.
#[derive(Debug, Clone)]
pub struct FrameCache {
    entries: std::collections::HashMap<FrameKey, Crop>,
    order: std::collections::VecDeque<FrameKey>,
    capacity: usize,
    pub hits: u32,
    pub misses: u32,
}

/// The phone's in-memory capacity: 64 crops, comfortably more than one product's timeline.
pub const FRAME_CACHE_CAPACITY: usize = 64;

impl Default for FrameCache {
    fn default() -> Self {
        Self::new(FRAME_CACHE_CAPACITY)
    }
}

impl FrameCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
        }
    }

    pub fn get(&mut self, key: &FrameKey) -> Option<Crop> {
        match self.entries.get(key) {
            Some(crop) => {
                self.hits += 1;
                Some(crop.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn insert(&mut self, key: FrameKey, crop: Crop) {
        if self.entries.insert(key.clone(), crop).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.capacity {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// [`crop_frame`] through a [`FrameCache`]: a hit costs no request at all.
pub fn crop_frame_cached<H: Http>(
    http: &mut H,
    origin: &str,
    frame: &ShardRead,
    corridor: &Bbox,
    cache: &mut FrameCache,
) -> Result<Crop, CropError> {
    // The window comes from the derived geometry, so the lookup happens before the header read
    // it would otherwise have to pay for. `crop_frame` still re-derives it from the fetched header
    // and refuses any disagreement, so a manifest cannot steer a crop it does not match.
    let Some(window) = window_of(&frame.geometry, corridor) else {
        return crop_frame(http, origin, frame, corridor);
    };
    let key = FrameKey::new(&frame.key, &window);
    if let Some(crop) = cache.get(&key) {
        return Ok(crop);
    }
    let crop = crop_frame(http, origin, frame, corridor)?;
    cache.insert(key, crop.clone());
    Ok(crop)
}
