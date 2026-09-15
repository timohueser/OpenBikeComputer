//! Elevation bands and grades sampled once when a route loads.
//!
//! The profile stores the finest min/max columns. Zoomed-out samples merge at most
//! eight adjacent columns, so every zoom level preserves extrema and unknown gaps
//! without storing duplicate bands or reading route geometry during rendering.

use heapless::Vec;

use crate::reader::{decode_chunk_from, parse_chunk_meta, read_header, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_elevation::DeadBand;
use obc_formats::io::{ByteSource, Error};
use obc_formats::obcr::CHUNK_META_LEN;
use obc_map_scene::ground_dist_m;

/// Finest profile resolution; supports one zoom step over the 240-pixel panel.
pub const PROFILE_COLS: usize = 512;
const LEVEL_COLS: [usize; 4] = [PROFILE_COLS, PROFILE_COLS / 2, PROFILE_COLS / 4, PROFILE_COLS / 8];
const NUM_LEVELS: usize = LEVEL_COLS.len();
/// Cumulative ascent serves the live remaining-climb statistic.
const ASCENT_COLS: usize = 256;

/// The visible slice of the profile a zoomed/panned view should draw: which pyramid
/// `level` to read and the fractional `[lo_frac, hi_frac]` route span it covers. Returned
/// by [`Profile::window`]; the screen maps each chart pixel to a fraction in this span and
/// reads the band via [`Profile::sample`] at `level`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Pyramid level to sample (0 = finest). Picked so the window holds ≥ the chart's
    /// pixel width in columns — enough detail without walking more than ~chart-width.
    pub level: usize,
    /// Fractional start of the visible route span (`0.0` = route start).
    pub lo_frac: f32,
    /// Fractional end of the visible route span (`1.0` = route end).
    pub hi_frac: f32,
}

/// A route's elevation bands, y-axis range, peak, and cumulative-ascent curve —
/// everything the Statistics screen draws at any zoom without re-reading the route. Build with
/// [`RouteReader::elevation_profile`] and cache it.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Base min/max columns. `min > max` marks unknown elevation.
    cols: [(i16, i16); PROFILE_COLS],
    /// Cumulative route ascent (m) through each column — monotonic non-decreasing,
    /// normalized so the last column equals the route's total ascent. Computed from the
    /// per-point elevations at [`ASCENT_COLS`] resolution (not the coarse per-chunk
    /// samples), so "to climb" is correct even on a route with few chunks.
    cum_ascent: [u32; ASCENT_COLS],
    grades: [i8; PROFILE_COLS],
    /// Lowest/highest elevation over the whole route (the y-axis range), from the route
    /// header. Equal for a perfectly flat route — callers guard the zero-height span.
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    /// Base-level column of the highest point, for placing the peak label / readout.
    pub peak_col: usize,
}

impl Profile {
    /// Empty storage for hosts that build a profile directly into their resident cache.
    /// [`ride_track_into`] resets every field before filling it.
    pub const EMPTY: Self = Profile {
        cols: [(i16::MAX, i16::MIN); PROFILE_COLS],
        cum_ascent: [0; ASCENT_COLS],
        grades: [i8::MIN; PROFILE_COLS],
        min_ele_m: 0,
        max_ele_m: 0,
        peak_col: 0,
    };

    fn reset(&mut self) {
        self.cols.fill((i16::MAX, i16::MIN));
        self.cum_ascent.fill(0);
        self.grades.fill(i8::MIN);
        self.min_ele_m = 0;
        self.max_ele_m = 0;
        self.peak_col = 0;
    }

    /// The base (finest) level's per-column `(min, max)` elevations — always
    /// [`PROFILE_COLS`] long. The fully-detailed band; zoomed views use [`sample`] /
    /// [`window`] instead so they can read a coarser level when zoomed out.
    ///
    /// [`sample`]: Profile::sample
    /// [`window`]: Profile::window
    #[inline]
    pub fn cols(&self) -> &[(i16, i16)] {
        &self.cols
    }

    /// The `(min, max)` elevation at fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) on the **base** level — for the "you are here"
    /// cursor's readout and the grade window.
    #[inline]
    pub fn at(&self, t: f32) -> (i16, i16) {
        self.sample(0, t)
    }

    /// The `(min, max)` elevation at fractional position `t` on a given pyramid `level`
    /// — the zoom-aware read the screen uses to draw a [`Window`]'s band column by column.
    #[inline]
    pub fn sample(&self, level: usize, t: f32) -> (i16, i16) {
        let level = level.min(NUM_LEVELS - 1);
        let last = LEVEL_COLS[level] - 1;
        let col = ((t.clamp(0.0, 1.0) * last as f32) as usize).min(last);
        let width = 1 << level;
        let bands = &self.cols[col * width..(col + 1) * width];
        let mut result = bands[0];
        for &(min, max) in bands {
            if min > max {
                return (i16::MAX, i16::MIN);
            }
            result = (result.0.min(min), result.1.max(max));
        }
        result
    }

    pub fn grade_at(&self, frac: f32) -> Option<i32> {
        let col = (frac.clamp(0.0, 1.0) * (PROFILE_COLS - 1) as f32) as usize;
        let grade = self.grades[col];
        (grade != i8::MIN).then_some(i32::from(grade))
    }

    /// Peak elevation in meters (the max at [`peak_col`](Profile::peak_col)).
    #[inline]
    pub fn peak_ele_m(&self) -> i16 {
        self.cols[self.peak_col].1
    }

    /// The peak's fractional position along the route (`0.0`–`1.0`) — for placing the
    /// peak readout relative to the live cursor regardless of zoom.
    #[inline]
    pub fn peak_frac(&self) -> f32 {
        self.peak_col as f32 / (PROFILE_COLS - 1) as f32
    }

    /// Cumulative ascent (m) climbed by fractional position `t` along the route
    /// (`0.0` = start, `1.0` = end) — for "to climb" (`total_ascent - ascent_to`).
    /// Interpolated at [`ASCENT_COLS`] resolution and normalized so `ascent_to(1.0)` is
    /// exactly the route's total ascent.
    #[inline]
    pub fn ascent_to(&self, t: f32) -> u32 {
        let last = ASCENT_COLS - 1;
        let x = t.clamp(0.0, 1.0) * last as f32;
        let i = x as usize;
        if i >= last {
            return self.cum_ascent[last];
        }
        let f = x - i as f32;
        let a = self.cum_ascent[i] as f32;
        let b = self.cum_ascent[i + 1] as f32;
        (a + (b - a) * f) as u32
    }

    /// Cumulative ascent (m) climbed by **`dist_m` metres** along a route of `route_total_m` — the
    /// distance-indexed twin of [`ascent_to`](Self::ascent_to), which every consumer that thinks in
    /// route metres (matched ride progress, a waypoint's `dist_along_m`, a corridor POI's
    /// along-route position) wants instead of a fraction it has to derive itself.
    ///
    /// A zero-length route has no axis to place `dist_m` on, so it reads `0`.
    #[inline]
    pub fn ascent_to_m(&self, dist_m: u32, route_total_m: u32) -> u32 {
        if route_total_m == 0 {
            return 0;
        }
        self.ascent_to((dist_m.min(route_total_m) as f32 / route_total_m as f32).clamp(0.0, 1.0))
    }

    /// Ascent (m) still to climb between two along-route distances — `ascent_to_m(to) −
    /// ascent_to_m(from)`, saturating so a backwards pair reads `0` rather than wrapping.
    ///
    /// This is the one "climb between here and there" lookup: the Up-ahead rows' climb-to-go
    /// (`from` = matched progress, `to` = the entry's `dist_along_m`), the `TO CLIMB` tile and the
    /// ETA model's ascent-to-go (`to` = `route_total_m`) all read it, so they cannot drift apart.
    /// Non-increasing in `from` and non-decreasing in `to`, since the curve is monotonic.
    #[inline]
    pub fn ascent_between_m(&self, from_m: u32, to_m: u32, route_total_m: u32) -> u32 {
        self.ascent_to_m(to_m, route_total_m).saturating_sub(self.ascent_to_m(from_m, route_total_m))
    }

    /// Pick the pyramid [`Window`] to draw for a view centered on `center_frac` at zoom
    /// factor `zoom` (`1.0` = whole route, larger = closer), into a chart `target_px`
    /// wide.
    ///
    /// The visible span is `1/zoom` of the route, clamped to stay within `[0, 1]`. The
    /// level is the **coarsest** one that still puts at least `target_px` columns inside
    /// that span — so the draw has a source column per pixel without walking more than
    /// ~`2·target_px`. Pure arithmetic over cached bands: no geometry is read, so
    /// this is cheap to call per step.
    pub fn window(&self, center_frac: f32, zoom: f32, target_px: u32) -> Window {
        let zoom = zoom.max(1.0);
        let span = (1.0 / zoom).min(1.0);
        let half = span * 0.5;
        // Clamp the centre so the fixed-width span never runs off either end.
        let lo = (center_frac - half).clamp(0.0, 1.0 - span);
        let hi = (lo + span).min(1.0);

        // Coarsest level first; the first that holds ≥ target_px columns in the span wins
        // (fewest columns to walk at adequate detail). Falls through to the finest level
        // when zoomed in past what even the base resolves.
        let mut level = 0;
        for l in (0..NUM_LEVELS).rev() {
            if span * LEVEL_COLS[l] as f32 >= target_px as f32 {
                level = l;
                break;
            }
        }
        Window { level, lo_frac: lo, hi_frac: hi }
    }
}

impl RouteReader<'_> {
    /// Build the route's elevation [`Profile`] by streaming every chunk in order and
    /// bucketing each point into a base-level column by its cumulative distance from the
    /// start. Coarser levels are merged when sampled.
    ///
    /// Each chunk re-anchors to its stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) and accumulates per-segment
    /// distance within the chunk using the same metric the converter did, so column
    /// placement matches the format's distance exactly and can't drift over a long
    /// route. O(points), reading each chunk once — cache the result, don't call it per
    /// frame.
    pub fn elevation_profile(&self) -> Profile {
        // Sentinel for "no point landed here": an empty column has min > max. Only the
        // base level is filled by the sweep; coarser bands are merged when sampled.
        let mut cols = [(i16::MAX, i16::MIN); PROFILE_COLS];
        let mut gaps = [false; PROFILE_COLS];
        let mut grades = [i8::MIN; PROFILE_COLS];
        let mut previous_sample: Option<(RoutePoint, usize)> = None;
        // Running dead-banded ascent recorded at the last point of each ascent column
        // (0 = none yet); carried forward and scaled into `cum_ascent` below.
        let mut casc = [0f32; ASCENT_COLS];
        let total = self.total_distance_m.max(1) as f64;
        let base_last = PROFILE_COLS - 1;
        let asc_last = ASCENT_COLS - 1;

        // One sweep over the whole route: bucket each point into its distance column,
        // updating that column's elevation band and the continuous ascent integrator. The
        // integrator runs *across* chunk seams (a chunk's shared seam point compares equal
        // to itself, contributing nothing), so it stays one continuous pass.
        let mut ascent = DeadBand::<f32>::new();
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let n = self.chunks().len();
        for k in 0..n {
            if self.decode_chunk(k, &mut buf).is_err() {
                return Profile::EMPTY;
            }
            // The chunk's first point sits at its cumulative distance; the rest add up
            // segment by segment from there. Like the converter, accumulate the small
            // per-segment `f32` distances into an `f64` running total so a long route's
            // column placement can't drift (the two must match exactly — same metric).
            let mut dist = self.chunks()[k].cum_distance_m as f64;
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += ground_dist_m(pr, (p.lon, p.lat)) as f64;
                }
                prev = Some((p.lon, p.lat));
                let frac = dist / total;
                let col = ((frac * base_last as f64) as usize).min(base_last);
                if let Some((a, prev_col)) = previous_sample {
                    let known = !p.elevation_incomplete && a.elevation().is_some() && p.elevation().is_some();
                    let length = ground_dist_m((a.lon, a.lat), (p.lon, p.lat));
                    if length > 0.0 {
                        let grade =
                            libm::roundf((p.ele as f32 - a.ele as f32) * 100.0 / length).clamp(-127.0, 127.0) as i8;
                        for c in prev_col..=col {
                            if known {
                                grades[c] = grade;
                            } else {
                                gaps[c] = true;
                                grades[c] = i8::MIN;
                            }
                        }
                    }
                }
                previous_sample = Some((*p, col));
                if p.elevation().is_none() {
                    gaps[col] = true;
                    ascent.pause();
                    continue;
                }
                let slot = &mut cols[col];
                slot.0 = slot.0.min(p.ele);
                slot.1 = slot.1.max(p.ele);
                // Record the running ascent at this column (later points in the same column
                // overwrite, so it ends on the correct value).
                let acol = ((frac * asc_last as f64) as usize).min(asc_last);
                if p.elevation_incomplete {
                    ascent.pause();
                }
                ascent.push(p.ele as f32);
                casc[acol] = ascent.ascent();
            }
        }

        fill_gaps(&mut cols[..PROFILE_COLS], band_fallback((self.min_ele_m, self.max_ele_m)), band_is_set);
        for (i, gap) in gaps.into_iter().enumerate() {
            if gap {
                cols[i] = (i16::MAX, i16::MIN);
                grades[i] = i8::MIN;
            }
        }
        let cum_ascent = cumulative_ascent(&casc, self.total_ascent_m);
        let peak_col = peak_column(&cols[..PROFILE_COLS]);

        Profile { cols, cum_ascent, grades, min_ele_m: self.min_ele_m, max_ele_m: self.max_ele_m, peak_col }
    }
}

/// Fill a recorded ride's elevation [`Profile`] and preview from one pass over its 20-byte samples.
///
/// The route twin is [`RouteReader::elevation_profile`]; this shares its whole tail (gap-fill,
/// cumulative ascent, peak) and differs only in the sweep:
/// - points are the final object's 20-byte records (`lon, lat` in microdegrees, exactly as they
///   were recorded); the fixed summary footer is not part of the sweep;
/// - columns bucket by the accumulated segment distance over the **header's** `distance` total
///   (the one total knowable in a single pass; the tail past it clamps into the last column and
///   any unreached columns gap-fill);
/// - the y-range is the sweep's own min/max (the ride header stores none) and the ascent curve
///   normalizes to the header's `climb` total.
///
/// Fill the caller's profile and preview together, reading the footer once and each 32-record
/// block (640 B) once. The preview keeps at most `N` uniformly spaced point indices, including
/// both endpoints, as `(lon, lat)` microdegrees. No whole-track buffer or by-value profile is
/// allocated: the board fills its resident profile without growing its task frame.
///
/// On error, the preview is empty and the partially filled profile must not be published.
/// Rejects what [`RideInfo::read`](crate::RideInfo::read) rejects (bad version, torn length).
pub fn ride_track_into<const N: usize>(
    src: &dyn ByteSource,
    out: &mut Profile,
    preview: &mut Vec<(i32, i32), N>,
) -> Result<(), Error> {
    use obc_formats::ride::SAMPLE_LEN;

    preview.clear();
    let info = crate::RideInfo::read(src)?;
    let total_points = info.point_count as usize;
    let keep = N.min(total_points);
    let mut next = 0usize;

    // Build the band **into the result value**, not a separate `cols` scratch: the array is
    // `PROFILE_COLS × 4 B` and moving a local into the returned `Profile` at the end leaves both
    // live in the frame at once. Written in place it exists once (the ascent curve stays a local
    // — it integrates as `f32` and is quantised into the struct's `u32` at the end).
    out.reset();
    let mut casc = [0f32; ASCENT_COLS];
    let total = info.distance_m.max(1) as f64;
    let base_last = PROFILE_COLS - 1;
    let asc_last = ASCENT_COLS - 1;
    let (mut min_ele, mut max_ele) = (i16::MAX, i16::MIN);

    // One sweep over the point records, a block per read — the distance runs through elevation
    // gaps (a no-ele point still moves the rider), the ascent integrator only over real samples.
    let mut ascent = DeadBand::<f32>::new();
    let mut dist = 0f64;
    let mut prev: Option<(i32, i32)> = None;
    const BLOCK: usize = 32;
    let mut buf = [0u8; BLOCK * SAMPLE_LEN];
    let mut done: u32 = 0;
    while done < info.point_count {
        let n = ((info.point_count - done) as usize).min(BLOCK);
        let bytes = &mut buf[..n * SAMPLE_LEN];
        if let Err(error) = src.read_at(u64::from(done) * SAMPLE_LEN as u64, bytes) {
            preview.clear();
            return Err(error);
        }
        for (i, rec) in bytes.as_chunks::<SAMPLE_LEN>().0.iter().enumerate() {
            let lat = i32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]);
            let lon = i32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            let ele = i16::from_le_bytes([rec[8], rec[9]]);
            let p = (lon, lat);
            if preview.len() < keep && done as usize + i == next {
                let _ = preview.push(p);
                if preview.len() < keep {
                    // Uniform point indices, including both endpoints (keep >= 2 here).
                    next = preview.len() * (total_points - 1) / (keep - 1);
                }
            }
            if let Some(pr) = prev {
                dist += ground_dist_m(pr, p) as f64;
            }
            prev = Some(p);
            min_ele = min_ele.min(ele);
            max_ele = max_ele.max(ele);
            let frac = dist / total;
            let col = ((frac * base_last as f64) as usize).min(base_last);
            let slot = &mut out.cols[col];
            slot.0 = slot.0.min(ele);
            slot.1 = slot.1.max(ele);
            let acol = ((frac * asc_last as f64) as usize).min(asc_last);
            ascent.push(ele as f32);
            casc[acol] = ascent.ascent();
        }
        done += n as u32;
    }

    // A ride with no elevation at all (every point the sentinel): a flat zero band, not i16 junk.
    if min_ele > max_ele {
        (min_ele, max_ele) = (0, 0);
    }
    fill_gaps(&mut out.cols[..PROFILE_COLS], band_fallback((min_ele, max_ele)), band_is_set);
    out.cum_ascent = cumulative_ascent(&casc, info.climb_m as u32);
    out.peak_col = peak_column(&out.cols[..PROFILE_COLS]);
    out.min_ele_m = min_ele;
    out.max_ele_m = max_ele;
    Ok(())
}

/// Buckets in the received-route card's mini elevation sparkline (#682): one min–max-normalized
/// `u8` height per bucket, sampled left-to-right along the route. Small and fixed so the
/// route-upload seam can carry the whole band by value with the event.
pub const SPARKLINE_BUCKETS: usize = 64;

/// Build the received-route card's mini elevation sparkline by streaming the route **once**:
/// bucket every point into one of [`SPARKLINE_BUCKETS`] distance columns (keeping each column's
/// peak height), fill any column no point landed in from its neighbour, then min–max-normalize the
/// columns to `u8`. Returns `None` for a flat range, incomplete elevation, or an unreadable
/// chunk: this compact band cannot represent a gap.
///
/// Column placement mirrors [`RouteReader::elevation_profile`] (re-anchor each chunk to its
/// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m), accumulate per-segment distance from
/// there), so the mini band reads as a coarser copy of the full Route-overview band. `O(points)`,
/// one pass over the geometry — call it once at commit time on the host, never on the render path.
pub fn elevation_sparkline(src: &dyn ByteSource) -> Option<[u8; SPARKLINE_BUCKETS]> {
    // **Streams the chunk index; never materialises it.** A `RouteIndex` is
    // `MAX_ROUTE_CHUNKS × 48 B` and is returned by value, so building one here put tens of KB on
    // the stack to produce this function's 64-byte result (73.7 KB measured on the LM20 at 512
    // chunks — more than the whole stack region; issue: LM20 retarget, 2026-07-24). Nothing here
    // needs random access: the walk is strictly forward, one chunk at a time, so it reads each
    // 48-byte meta straight from the source through the same `parse_chunk_meta` the index build
    // uses. Resident cost is now the point scratch alone, independent of `MAX_ROUTE_CHUNKS`.
    let h = read_header(src).ok()?;
    let lo = h.min_ele_m as i32;
    let span = h.max_ele_m as i32 - lo;
    if span <= 0 {
        return None; // flat / no elevation — omit the band
    }
    let total = h.total_distance_m.max(1) as f64;
    let last = SPARKLINE_BUCKETS - 1;
    // Peak height per bucket; sentinel `i16::MIN` = "no point landed here" (gap-filled below).
    let mut maxes = [i16::MIN; SPARKLINE_BUCKETS];
    let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
    let mut meta_bytes = [0u8; CHUNK_META_LEN];
    let src_len = src.len();
    for k in 0..h.chunk_count {
        let off = h.index_offset + k * CHUNK_META_LEN as u32;
        src.read_at(off.into(), &mut meta_bytes).ok()?;
        let m = parse_chunk_meta(&meta_bytes, src_len).ok()?;
        let n = m.point_count as usize;
        buf.clear();
        if n == 0 {
            return None;
        }
        decode_chunk_from(src, &m, n, &mut buf).ok()?;
        let mut dist = m.cum_distance_m as f64;
        let mut prev: Option<(i32, i32)> = None;
        for p in &buf {
            if let Some(pr) = prev {
                dist += ground_dist_m(pr, (p.lon, p.lat)) as f64;
            }
            prev = Some((p.lon, p.lat));
            let b = ((dist / total) * last as f64) as usize;
            let b = b.min(last);
            p.elevation()?;
            if p.elevation_incomplete {
                return None;
            }
            if p.ele > maxes[b] {
                maxes[b] = p.ele;
            }
        }
    }
    // Carry the last filled height across empty buckets (sparse geometry can skip one), forward
    // then backward for any leading gap — the profile's gap-fill, one channel.
    let mut carry: Option<i16> = None;
    for m in maxes.iter_mut() {
        match carry {
            Some(c) if *m == i16::MIN => *m = c,
            _ => carry = Some(*m),
        }
    }
    let mut back: Option<i16> = None;
    for m in maxes.iter_mut().rev() {
        match back {
            Some(b) if *m == i16::MIN => *m = b,
            _ => back = Some(*m),
        }
    }
    let mut out = [0u8; SPARKLINE_BUCKETS];
    for (o, &m) in out.iter_mut().zip(maxes.iter()) {
        *o = (((m as i32 - lo) * 255 / span).clamp(0, 255)) as u8;
    }
    Some(out)
}

/// Turn the per-column running ascent (`casc`, set only where points landed) into a
/// gap-free, monotonic-non-decreasing cumulative-ascent curve, scaled so the final column
/// equals the header's exact `total_ascent_m` (so "to climb" reaches 0 at the route's end).
fn cumulative_ascent(casc: &[f32; ASCENT_COLS], total_ascent_m: u32) -> [u32; ASCENT_COLS] {
    let last_col = ASCENT_COLS - 1;
    // Carry the running value across empty columns, keeping the curve non-decreasing.
    let mut raw = [0f32; ASCENT_COLS];
    let mut run = 0f32;
    for i in 0..ASCENT_COLS {
        run = run.max(casc[i]);
        raw[i] = run;
    }
    // Scale to the header's exact total, then pin the endpoint so rounding can't miss it.
    let mut cum = [0u32; ASCENT_COLS];
    if raw[last_col] > 0.0 {
        let scale = total_ascent_m as f32 / raw[last_col];
        for i in 0..ASCENT_COLS {
            cum[i] = (raw[i] * scale) as u32;
        }
    }
    cum[last_col] = total_ascent_m;
    cum
}

/// The column index of the route's highest point (for placing the peak label).
fn peak_column(cols: &[(i16, i16)]) -> usize {
    let mut peak_col = 0;
    let mut peak = i16::MIN;
    for (i, c) in cols.iter().enumerate() {
        if c.1 > peak {
            peak = c.1;
            peak_col = i;
        }
    }
    peak_col
}

/// Make `cols` gap-free: each empty column inherits the nearest filled column — forward carry
/// first, then a backward carry for any leading run of empties the forward pass can't reach. A
/// column still empty after both falls back to `fallback`, so the buffer is never left holding a
/// sentinel.
///
/// Generic over the column payload and its emptiness test, because both elevation buffers want
/// exactly this carry: the route [`Profile`]'s `(min, max)` band (sentinel `min > max`, falling
/// back to the header extent so the band still has a shape when a route decodes to nothing) and
/// the [`ClimbProfile`](crate::climb_profile::ClimbProfile)'s one-sample-per-column scalar
/// (sentinel [`EMPTY`](crate::climb_profile), falling back to the seg's base).
pub(crate) fn fill_gaps<T: Copy>(cols: &mut [T], fallback: T, is_set: impl Fn(&T) -> bool) {
    let mut last: Option<T> = None;
    for c in cols.iter_mut() {
        if is_set(c) {
            last = Some(*c);
        } else if let Some(v) = last {
            *c = v;
        }
    }
    // Backward carry fills columns before the first set one (forward carry can't reach).
    let mut next: Option<T> = None;
    for c in cols.iter_mut().rev() {
        if is_set(c) {
            next = Some(*c);
        } else if let Some(v) = next {
            *c = v;
        }
    }
    // Only reachable when the whole span had no decodable points.
    for c in cols.iter_mut() {
        if !is_set(c) {
            *c = fallback;
        }
    }
}

/// The band buffer's emptiness test: an unwritten column carries the inverted sentinel `min > max`.
fn band_is_set(c: &(i16, i16)) -> bool {
    c.0 <= c.1
}

/// Normalize a `(min, max)` band fallback so it reads as *set* — the header extent is trusted for
/// its two values, not for their order.
fn band_fallback(fallback: (i16, i16)) -> (i16, i16) {
    (fallback.0.min(fallback.1), fallback.0.max(fallback.1))
}
