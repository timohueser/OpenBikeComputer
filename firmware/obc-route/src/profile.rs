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

/// The slice of the profile a zoomed view draws: which pyramid level to read and the fractional
/// route span it covers. The screen maps each chart pixel to a fraction in the span and reads the
/// band with [`Profile::sample`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Pyramid level to sample, 0 being finest. It is picked so the window holds at least the
    /// chart's pixel width in columns.
    pub level: usize,
    pub lo_frac: f32,
    pub hi_frac: f32,
}

/// A route's elevation bands, y-axis range, peak and cumulative-ascent curve: everything the
/// Statistics screen draws at any zoom without re-reading the route. Build it once and cache it.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Base min/max columns. `min > max` marks unknown elevation.
    cols: [(i16, i16); PROFILE_COLS],
    /// Cumulative ascent (m) through each column, non-decreasing and normalized so the last
    /// column equals the route's total ascent. It comes from the per-point elevations, not the
    /// coarse per-chunk samples, so "to climb" is correct on a route with few chunks.
    cum_ascent: [u32; ASCENT_COLS],
    grades: [i8; PROFILE_COLS],
    /// The y-axis range, from the route header. The two are equal on a flat route, so callers
    /// guard the zero-height span.
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    /// Base-level column of the highest point, for placing the peak label.
    pub peak_col: usize,
}

impl Profile {
    /// Empty storage for a host that builds a profile into its resident cache.
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

    /// The finest level's per-column `(min, max)` elevations. A zoomed view uses
    /// [`sample`](Profile::sample) and [`window`](Profile::window) instead, so it can read a
    /// coarser level.
    #[inline]
    pub fn cols(&self) -> &[(i16, i16)] {
        &self.cols
    }

    /// The `(min, max)` elevation at fractional position `t` on the finest level.
    #[inline]
    pub fn at(&self, t: f32) -> (i16, i16) {
        self.sample(0, t)
    }

    /// The `(min, max)` elevation at fractional position `t` on a pyramid level: the zoom-aware
    /// read the screen draws a [`Window`] with.
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

    #[inline]
    pub fn peak_ele_m(&self) -> i16 {
        self.cols[self.peak_col].1
    }

    /// The peak's fractional position along the route, for placing its readout at any zoom.
    #[inline]
    pub fn peak_frac(&self) -> f32 {
        self.peak_col as f32 / (PROFILE_COLS - 1) as f32
    }

    /// Cumulative ascent (m) climbed by fractional position `t`. It is normalized so
    /// `ascent_to(1.0)` is exactly the route's total ascent.
    #[inline]
    pub fn ascent_to(&self, t: f32) -> u32 {
        ascent_at(&self.cum_ascent, t)
    }

    /// The distance-indexed twin of [`ascent_to`](Self::ascent_to), for every consumer that
    /// thinks in route meters. A zero-length route has no axis, so it reads `0`.
    #[inline]
    pub fn ascent_to_m(&self, dist_m: u32, route_total_m: u32) -> u32 {
        if route_total_m == 0 {
            return 0;
        }
        self.ascent_to((dist_m.min(route_total_m) as f32 / route_total_m as f32).clamp(0.0, 1.0))
    }

    /// Ascent (m) between two along-route distances, saturating so a backwards pair reads `0`.
    /// This is the one "climb between here and there" lookup, so the Up-ahead rows, the TO CLIMB
    /// tile and the ETA model cannot drift apart.
    #[inline]
    pub fn ascent_between_m(&self, from_m: u32, to_m: u32, route_total_m: u32) -> u32 {
        self.ascent_to_m(to_m, route_total_m).saturating_sub(self.ascent_to_m(from_m, route_total_m))
    }

    /// Pick the [`Window`] to draw for a view centered on `center_frac` at zoom factor `zoom`,
    /// into a chart `target_px` wide.
    ///
    /// The level is the coarsest one that still puts at least `target_px` columns inside the
    /// visible span, so the draw has a source column per pixel without walking much more. It reads
    /// no geometry, so it is cheap to call per step.
    pub fn window(&self, center_frac: f32, zoom: f32, target_px: u32) -> Window {
        let zoom = zoom.max(1.0);
        let span = (1.0 / zoom).min(1.0);
        let half = span * 0.5;
        let lo = (center_frac - half).clamp(0.0, 1.0 - span);
        let hi = (lo + span).min(1.0);

        // Coarsest level first. It falls through to the finest level when zoomed in past what
        // the base resolves.
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
    /// Build the route's elevation [`Profile`] by streaming every chunk in order and bucketing
    /// each point into a base-level column by its cumulative distance. Coarser levels merge when
    /// sampled.
    ///
    /// Each chunk re-anchors to its stored
    /// [`cum_distance_m`](crate::ChunkMeta::cum_distance_m) and uses the same distance metric the
    /// converter did, so column placement matches the format exactly. Each chunk is read once, so
    /// cache the result rather than calling this per frame.
    pub fn elevation_profile(&self) -> Profile {
        // An empty column carries the sentinel `min > max`.
        let mut cols = [(i16::MAX, i16::MIN); PROFILE_COLS];
        let mut gaps = [false; PROFILE_COLS];
        let mut grades = [i8::MIN; PROFILE_COLS];
        let mut previous_sample: Option<(RoutePoint, usize)> = None;
        // The running ascent at the last point of each ascent column, carried forward and scaled
        // into `cum_ascent` below.
        let mut casc = [0f32; ASCENT_COLS];
        let total = self.total_distance_m.max(1) as f64;
        let base_last = PROFILE_COLS - 1;
        let asc_last = ASCENT_COLS - 1;

        // The integrator runs across chunk seams: a shared seam point compares equal to itself
        // and contributes nothing, so this stays one continuous pass.
        let mut ascent = DeadBand::<f32>::new();
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let n = self.chunks().len();
        for k in 0..n {
            if self.decode_chunk(k, &mut buf).is_err() {
                return Profile::EMPTY;
            }
            // Like the converter, the small per-segment `f32` distances accumulate into an
            // `f64` total, so a long route's column placement cannot drift.
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
                // A later point in the same column overwrites this, so the column ends on the
                // correct value.
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

/// Fill a recorded ride's elevation [`Profile`] and preview from one pass over its samples.
///
/// It shares the gap-fill, cumulative ascent and peak of
/// [`RouteReader::elevation_profile`] and differs only in the sweep: columns bucket by the
/// accumulated segment distance over the header's own distance total, which is the one total
/// knowable in a single pass, and the y-range is the sweep's own min and max, because the ride
/// header stores none.
///
/// The preview keeps at most `N` uniformly spaced point indices, both endpoints included. No
/// whole-track buffer or by-value profile is allocated, so the board fills its resident profile
/// without growing its task frame.
///
/// On an error the preview is empty and the partly filled profile must not be published.
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

    // The band is built into the result value, not a `cols` scratch: moving a local into the
    // result would leave both live in the frame at once. The ascent curve stays a local because
    // it integrates as `f32` and is quantised at the end.
    out.reset();
    let mut casc = [0f32; ASCENT_COLS];
    let total = info.distance_m.max(1) as f64;
    let base_last = PROFILE_COLS - 1;
    let asc_last = ASCENT_COLS - 1;
    let (mut min_ele, mut max_ele) = (i16::MAX, i16::MIN);

    // The distance runs through elevation gaps, because a point without a height still moves the
    // rider; the ascent integrator runs only over real samples.
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

    // A ride with no elevation at all reads as a flat zero band, not as sentinel values.
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

/// The elevation [`Profile`] of one day made of stretches of stored routes, ridden one after the
/// other: the rest of the day before, then the day. The host streams one route at a time into the
/// resident profile, so it never holds two routes or a second profile.
///
/// A stretch climbs what its route's own ascent curve says between its two ends. That curve is
/// normalised to the route header's total as [`RouteReader::elevation_profile`] does, so a whole
/// route climbs exactly its header figure.
pub struct DayProfile {
    length_m: f64,
    /// Where the next stretch starts on the day.
    offset_m: f64,
    /// The day's running ascent per ascent column, scaled to the stretches' climb at the end.
    casc: [f32; ASCENT_COLS],
    ascent: DeadBand<f32>,
    climb_m: u32,
    min_ele: i16,
    max_ele: i16,
}

impl DayProfile {
    /// Start a day `length_m` long in `out`.
    pub fn start(length_m: u32, out: &mut Profile) -> Self {
        out.reset();
        DayProfile {
            length_m: f64::from(length_m.max(1)),
            offset_m: 0.0,
            casc: [0.0; ASCENT_COLS],
            ascent: DeadBand::new(),
            climb_m: 0,
            min_ele: i16::MAX,
            max_ele: i16::MIN,
        }
    }

    /// Append `[from_m, to_m]` of the route in `src`. `to_m` clamps to the route's length.
    pub fn stretch(&mut self, src: &dyn ByteSource, from_m: u32, to_m: u32, out: &mut Profile) -> Result<(), Error> {
        let h = read_header(src)?;
        let total = f64::from(h.total_distance_m.max(1));
        let to = to_m.min(h.total_distance_m);
        let from = from_m.min(to);
        let (base_last, asc_last) = (PROFILE_COLS - 1, ASCENT_COLS - 1);
        let mut route_casc = [0f32; ASCENT_COLS];
        let mut route_ascent = DeadBand::<f32>::new();
        // The gap between two stretches is not climbed.
        self.ascent.pause();
        let mut buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK> = Vec::new();
        let mut meta_bytes = [0u8; CHUNK_META_LEN];
        for k in 0..h.chunk_count {
            let off = k.checked_mul(CHUNK_META_LEN as u32).and_then(|rel| h.index_offset.checked_add(rel));
            src.read_at(off.ok_or(Error::BadOffset)?.into(), &mut meta_bytes)?;
            let m = parse_chunk_meta(&meta_bytes, src.len())?;
            buf.clear();
            decode_chunk_from(src, &m, m.point_count as usize, &mut buf)?;
            let mut dist = f64::from(m.cum_distance_m);
            let mut prev: Option<(i32, i32)> = None;
            for p in &buf {
                if let Some(pr) = prev {
                    dist += ground_dist_m(pr, (p.lon, p.lat)) as f64;
                }
                prev = Some((p.lon, p.lat));
                if p.elevation().is_none() {
                    route_ascent.pause();
                    self.ascent.pause();
                    continue;
                }
                if p.elevation_incomplete {
                    route_ascent.pause();
                    self.ascent.pause();
                }
                route_ascent.push(p.ele as f32);
                route_casc[((dist / total * asc_last as f64) as usize).min(asc_last)] = route_ascent.ascent();
                if dist < f64::from(from) || dist > f64::from(to) {
                    continue;
                }
                let frac = (self.offset_m + dist - f64::from(from)) / self.length_m;
                let slot = &mut out.cols[((frac * base_last as f64) as usize).min(base_last)];
                slot.0 = slot.0.min(p.ele);
                slot.1 = slot.1.max(p.ele);
                self.min_ele = self.min_ele.min(p.ele);
                self.max_ele = self.max_ele.max(p.ele);
                self.ascent.push(p.ele as f32);
                self.casc[((frac * asc_last as f64) as usize).min(asc_last)] = self.ascent.ascent();
            }
        }
        let curve = cumulative_ascent(&route_casc, h.total_ascent_m);
        let at = |m: u32| ascent_at(&curve, (f64::from(m) / total) as f32);
        self.climb_m += at(to).saturating_sub(at(from));
        self.offset_m += f64::from(to - from);
        Ok(())
    }

    /// Finish `out`, and return the day's climb.
    pub fn finish(self, out: &mut Profile) -> u32 {
        let (min_ele, max_ele) = if self.min_ele > self.max_ele { (0, 0) } else { (self.min_ele, self.max_ele) };
        fill_gaps(&mut out.cols[..PROFILE_COLS], (min_ele, max_ele), band_is_set);
        out.cum_ascent = cumulative_ascent(&self.casc, self.climb_m);
        out.peak_col = peak_column(&out.cols[..PROFILE_COLS]);
        out.min_ele_m = min_ele;
        out.max_ele_m = max_ele;
        self.climb_m
    }
}

/// The ascent at fraction `t` of a cumulative ascent curve, interpolated between its columns.
fn ascent_at(cum: &[u32; ASCENT_COLS], t: f32) -> u32 {
    let last = ASCENT_COLS - 1;
    let x = t.clamp(0.0, 1.0) * last as f32;
    let i = x as usize;
    if i >= last {
        return cum[last];
    }
    let f = x - i as f32;
    let (a, b) = (cum[i] as f32, cum[i + 1] as f32);
    (a + (b - a) * f) as u32
}

/// Buckets in the received-route card's mini elevation sparkline: one normalized `u8` height each.
/// Small and fixed, so the route-upload seam carries the whole band by value with the event.
pub const SPARKLINE_BUCKETS: usize = 64;

/// Build the received-route card's mini elevation sparkline by streaming the route once: bucket
/// every point into one of [`SPARKLINE_BUCKETS`] distance columns, keeping each column's peak,
/// fill any empty column from its neighbour, then normalize to `u8`. `None` for a flat range,
/// incomplete elevation or an unreadable chunk, because this compact band cannot hold a gap.
///
/// Column placement matches [`RouteReader::elevation_profile`], so the mini band reads as a
/// coarser copy of the full one. Call it once at commit time, never on the render path.
pub fn elevation_sparkline(src: &dyn ByteSource) -> Option<[u8; SPARKLINE_BUCKETS]> {
    // This streams the chunk index and never materialises it. A `RouteIndex` is returned by
    // value, so building one here would put tens of kB on the stack for a 64-byte result. The
    // walk is strictly forward, so it reads each meta straight from the source. The resident cost
    // is the point scratch alone, independent of `MAX_ROUTE_CHUNKS`.
    let h = read_header(src).ok()?;
    let lo = h.min_ele_m as i32;
    let span = h.max_ele_m as i32 - lo;
    if span <= 0 {
        return None; // flat or no elevation: omit the band
    }
    let total = h.total_distance_m.max(1) as f64;
    let last = SPARKLINE_BUCKETS - 1;
    // Peak height per bucket. `i16::MIN` marks a bucket no point landed in.
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
    // Carry the last filled height across empty buckets, forward and then backward for a leading
    // gap: the profile's gap-fill over one channel.
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

/// Turn the per-column running ascent, which is set only where points landed, into a gap-free
/// non-decreasing curve scaled so the final column is exactly `total_ascent_m`. That makes "to
/// climb" reach 0 at the route end.
fn cumulative_ascent(casc: &[f32; ASCENT_COLS], total_ascent_m: u32) -> [u32; ASCENT_COLS] {
    let last_col = ASCENT_COLS - 1;
    // Carry the running value across empty columns, keeping the curve non-decreasing.
    let mut raw = [0f32; ASCENT_COLS];
    let mut run = 0f32;
    for i in 0..ASCENT_COLS {
        run = run.max(casc[i]);
        raw[i] = run;
    }
    // Pin the endpoint after scaling, so rounding cannot miss it.
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

/// Make `cols` gap-free: each empty column inherits the nearest filled one, forward first and
/// then backward for a leading run the forward pass cannot reach. A column still empty after both
/// takes `fallback`, so the buffer never keeps a sentinel.
///
/// It is generic over the payload and its emptiness test because both elevation buffers want this
/// carry: the route [`Profile`]'s band and the
/// [`ClimbProfile`](crate::climb_profile::ClimbProfile)'s per-column scalar.
pub(crate) fn fill_gaps<T: Copy>(cols: &mut [T], fallback: T, is_set: impl Fn(&T) -> bool) {
    let mut last: Option<T> = None;
    for c in cols.iter_mut() {
        if is_set(c) {
            last = Some(*c);
        } else if let Some(v) = last {
            *c = v;
        }
    }
    // The backward carry fills the columns before the first set one.
    let mut next: Option<T> = None;
    for c in cols.iter_mut().rev() {
        if is_set(c) {
            next = Some(*c);
        } else if let Some(v) = next {
            *c = v;
        }
    }
    // Only reached when the whole span had no decodable points.
    for c in cols.iter_mut() {
        if !is_set(c) {
            *c = fallback;
        }
    }
}

/// An unwritten column carries the inverted sentinel `min > max`.
fn band_is_set(c: &(i16, i16)) -> bool {
    c.0 <= c.1
}

/// Normalize a band fallback so it reads as set: the header extent is trusted for its two values,
/// not for their order.
fn band_fallback(fallback: (i16, i16)) -> (i16, i16) {
    (fallback.0.min(fallback.1), fallback.0.max(fallback.1))
}
