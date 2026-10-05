//! Elevation bands and grades sampled once when a route loads.
//!
//! The profile stores the finest min/max columns. Zoomed-out samples merge at most
//! eight adjacent columns, so every zoom level preserves extrema and unknown gaps
//! without storing duplicate bands or reading route geometry during rendering.

use heapless::Vec;

use crate::climb::{ClimbDetector, Climbs};
use crate::preview::Pick;
use crate::reader::{read_header, stored_meta, RoutePoint, RouteReader};
use crate::walk::{column, walk, Records};
use obc_elevation::DeadBand;
use obc_formats::io::{ByteSource, Error};
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

    /// # Safety
    /// `slot` must be aligned, writable and exclusively owned for a complete profile.
    pub unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        unsafe {
            for i in 0..PROFILE_COLS {
                addr_of_mut!((*slot).cols).cast::<(i16, i16)>().add(i).write((i16::MAX, i16::MIN));
            }
            addr_of_mut!((*slot).cum_ascent).write_bytes(0, 1);
            addr_of_mut!((*slot).grades).write_bytes(i8::MIN as u8, 1);
            addr_of_mut!((*slot).min_ele_m).write(0);
            addr_of_mut!((*slot).max_ele_m).write(0);
            addr_of_mut!((*slot).peak_col).write(0);
            let Self { cols: _, cum_ascent: _, grades: _, min_ele_m: _, max_ele_m: _, peak_col: _ } = &*slot;
        }
    }

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

    /// Widen the band of the column fraction `frac` of the profile falls in to hold `ele`.
    fn widen(&mut self, frac: f64, ele: i16) {
        let band = &mut self.cols[column(frac, PROFILE_COLS)];
        *band = (band.0.min(ele), band.1.max(ele));
    }

    /// Finish a sweep: fill the band's gaps from their neighbours, or from `(min, max)` where no
    /// column is set, then set the ascent curve, the peak and the y range.
    fn finish(&mut self, (min, max): (i16, i16), ascent: &Ascent, total_ascent_m: u32, gaps: &[bool]) {
        // The header extent is trusted for its two values, not for their order.
        fill_gaps(&mut self.cols, (min.min(max), min.max(max)), |c| c.0 <= c.1);
        for (i, _) in gaps.iter().enumerate().filter(|(_, gap)| **gap) {
            self.cols[i] = (i16::MAX, i16::MIN);
            self.grades[i] = i8::MIN;
        }
        self.cum_ascent = ascent.curve(total_ascent_m);
        let mut peak = i16::MIN;
        self.peak_col = 0;
        for (i, c) in self.cols.iter().enumerate() {
            if c.1 > peak {
                (peak, self.peak_col) = (c.1, i);
            }
        }
        (self.min_ele_m, self.max_ele_m) = (min, max);
    }
}

/// The running ascent at the last point of each ascent column, through the shared dead-band.
struct Ascent {
    band: DeadBand<f32>,
    cols: [f32; ASCENT_COLS],
}

impl Ascent {
    fn new() -> Self {
        Ascent { band: DeadBand::new(), cols: [0.0; ASCENT_COLS] }
    }

    fn pause(&mut self) {
        self.band.pause();
    }

    /// A later point in the same column overwrites this one, so each column ends on its last.
    fn push(&mut self, frac: f64, ele: i16) {
        self.band.push(ele as f32);
        self.cols[column(frac, ASCENT_COLS)] = self.band.ascent();
    }

    /// The curve, gap-free and non-decreasing, scaled so its last column is exactly
    /// `total_ascent_m`. That makes "to climb" reach 0 at the route end.
    fn curve(&self, total_ascent_m: u32) -> [u32; ASCENT_COLS] {
        let mut raw = [0f32; ASCENT_COLS];
        let mut run = 0f32;
        for (r, &c) in raw.iter_mut().zip(&self.cols) {
            run = run.max(c);
            *r = run;
        }
        // Pin the endpoint after scaling, so rounding cannot miss it.
        let mut cum = [0u32; ASCENT_COLS];
        if raw[ASCENT_COLS - 1] > 0.0 {
            let scale = total_ascent_m as f32 / raw[ASCENT_COLS - 1];
            for (c, r) in cum.iter_mut().zip(raw) {
                *c = (r * scale) as u32;
            }
        }
        cum[ASCENT_COLS - 1] = total_ascent_m;
        cum
    }
}

/// The route profile's fold over the walk: bands, grades, unknown-elevation gaps and ascent.
struct Sweep {
    total_m: f64,
    gaps: [bool; PROFILE_COLS],
    ascent: Ascent,
    previous: Option<(RoutePoint, usize)>,
}

impl Sweep {
    fn push(&mut self, profile: &mut Profile, p: RoutePoint, along_m: f64) {
        let frac = along_m / self.total_m;
        let col = column(frac, PROFILE_COLS);
        if let Some((a, previous_col)) = self.previous {
            let known = !p.elevation_incomplete && a.elevation().is_some() && p.elevation().is_some();
            let length = ground_dist_m((a.lon, a.lat), (p.lon, p.lat));
            if length > 0.0 {
                let grade = libm::roundf((p.ele as f32 - a.ele as f32) * 100.0 / length).clamp(-127.0, 127.0) as i8;
                for c in previous_col..=col {
                    profile.grades[c] = if known { grade } else { i8::MIN };
                    self.gaps[c] |= !known;
                }
            }
        }
        self.previous = Some((p, col));
        if p.elevation().is_none() {
            self.gaps[col] = true;
            self.ascent.pause();
            return;
        }
        profile.widen(frac, p.ele);
        if p.elevation_incomplete {
            self.ascent.pause();
        }
        self.ascent.push(frac, p.ele);
    }
}

impl RouteReader<'_> {
    /// Build the route's elevation [`Profile`] by streaming every chunk in order and bucketing
    /// each point into a base-level column by its along-route distance. Coarser levels merge when
    /// sampled. Each chunk is read once, so cache the result rather than calling this per frame.
    pub fn elevation_profile(&self) -> Profile {
        let mut profile = Profile::EMPTY;
        self.elevation_profile_into(&mut profile);
        profile
    }

    /// Fill resident profile storage without returning a large temporary.
    pub fn elevation_profile_into(&self, profile: &mut Profile) {
        self.summaries(Some(profile), false);
    }

    /// Derive both summaries from one geometry pass. After a read error, retry the profile
    /// independently so a transient failure in climb detection does not leave it empty.
    pub fn elevation_profile_and_climbs_into(&self, profile: &mut Profile) -> Climbs {
        let (climbs, profile_ok) = self.summaries(Some(profile), true);
        if !profile_ok {
            self.elevation_profile_into(profile);
        }
        climbs
    }

    /// One walk over the route into `profile`, and into the climb detector when `climbs` is set.
    /// A chunk that fails to decode empties the profile (`false`) and is left out of the climbs.
    // Out of line, so the sweep scratch is popped before a failed profile is retried.
    #[inline(never)]
    pub(crate) fn summaries(&self, mut profile: Option<&mut Profile>, climbs: bool) -> (Climbs, bool) {
        let mut sweep = profile.as_deref_mut().map(|profile| {
            profile.reset();
            Sweep {
                total_m: self.total_distance_m.max(1) as f64,
                gaps: [false; PROFILE_COLS],
                ascent: Ascent::new(),
                previous: None,
            }
        });
        let mut detector = climbs.then(ClimbDetector::new);
        let mut profile_ok = true;
        for (k, m) in self.chunks().iter().enumerate() {
            let read = self.with_chunk(k, |points| {
                walk(m.cum_distance_m, points).for_each(|step| {
                    if let Some(detector) = &mut detector {
                        detector.push_point(step.p, step.along);
                    }
                    if let (true, Some(sweep), Some(profile)) = (profile_ok, &mut sweep, profile.as_deref_mut()) {
                        sweep.push(profile, step.p, step.along);
                    }
                })
            });
            if read.is_err() {
                profile_ok = false;
                if detector.is_none() {
                    break;
                }
            }
        }
        let climbs = detector.map_or_else(Climbs::new, ClimbDetector::finish);
        if let (Some(profile), Some(sweep)) = (profile, sweep) {
            if !profile_ok {
                profile.reset();
            } else {
                profile.finish((self.min_ele_m, self.max_ele_m), &sweep.ascent, self.total_ascent_m, &sweep.gaps);
            }
        }
        (climbs, profile_ok)
    }
}

/// Fill a recorded ride's elevation [`Profile`], its [`RideTrackFacts`](crate::RideTrackFacts)
/// and its preview from one pass over its samples.
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
/// On an error the preview is empty and the partly filled profile and facts must not be published.
pub fn ride_track_into<const N: usize>(
    src: &dyn ByteSource,
    out: &mut Profile,
    facts: &mut crate::RideTrackFacts,
    preview: &mut Vec<(i32, i32), N>,
) -> Result<(), Error> {
    use obc_formats::ride::{HR_NONE, PWR_NONE, SAMPLE_LEN};

    preview.clear();
    let info = crate::RideInfo::read(src)?;
    let mut series = crate::ride::SeriesFill::start(facts, &info);
    let mut pick = Pick::new(info.point_count as usize, N);

    // The band is built into the result value, not a `cols` scratch: moving a local into the
    // result would leave both live in the frame at once.
    out.reset();
    let mut ascent = Ascent::new();
    let total = info.distance_m.max(1) as f64;
    let (mut min_ele, mut max_ele) = (i16::MAX, i16::MIN);

    // The distance runs through elevation gaps, because a point without a height still moves the
    // rider; the ascent integrator runs only over real samples.
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
            let hr = (rec[16] != HR_NONE).then_some(rec[16]);
            let power = u16::from_le_bytes([rec[18], rec[19]]);
            series.push(facts, done + i as u32, hr, (power != PWR_NONE).then_some(power));
            if pick.keep_next() {
                let _ = preview.push(p);
            }
            if let Some(pr) = prev {
                dist += ground_dist_m(pr, p) as f64;
            }
            prev = Some(p);
            min_ele = min_ele.min(ele);
            max_ele = max_ele.max(ele);
            out.widen(dist / total, ele);
            ascent.push(dist / total, ele);
        }
        done += n as u32;
    }
    series.finish(facts);

    // A ride with no elevation at all reads as a flat zero band, not as sentinel values.
    if min_ele > max_ele {
        (min_ele, max_ele) = (0, 0);
    }
    out.finish((min_ele, max_ele), &ascent, info.climb_m as u32, &[]);
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
    /// The day's running ascent, scaled to the stretches' climb at the end.
    ascent: Ascent,
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
            ascent: Ascent::new(),
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
        let mut route = Ascent::new();
        // The gap between two stretches is not climbed.
        self.ascent.pause();
        let mut records = Records::new();
        for k in 0..h.chunk_count {
            let m = stored_meta(src, &h, k)?;
            if m.point_count == 0 {
                return Err(Error::BadOffset);
            }
            records.read(src, &m)?;
            for step in walk(m.cum_distance_m, records.points()) {
                let (p, along) = (step.p, step.along);
                if p.elevation().is_none() || p.elevation_incomplete {
                    route.pause();
                    self.ascent.pause();
                }
                if p.elevation().is_none() {
                    continue;
                }
                route.push(along / total, p.ele);
                if along < f64::from(from) || along > f64::from(to) {
                    continue;
                }
                let frac = (self.offset_m + along - f64::from(from)) / self.length_m;
                out.widen(frac, p.ele);
                self.min_ele = self.min_ele.min(p.ele);
                self.max_ele = self.max_ele.max(p.ele);
                self.ascent.push(frac, p.ele);
            }
        }
        let curve = route.curve(h.total_ascent_m);
        let at = |m: u32| ascent_at(&curve, (f64::from(m) / total) as f32);
        self.climb_m += at(to).saturating_sub(at(from));
        self.offset_m += f64::from(to - from);
        Ok(())
    }

    /// Finish `out`, and return the day's climb.
    pub fn finish(self, out: &mut Profile) -> u32 {
        let range = if self.min_ele > self.max_ele { (0, 0) } else { (self.min_ele, self.max_ele) };
        out.finish(range, &self.ascent, self.climb_m, &[]);
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
/// coarser copy of the full one. It reads each chunk meta straight from the source, so it needs no
/// [`RouteIndex`](crate::RouteIndex). Call it once at commit time, never on the render path.
pub fn elevation_sparkline(src: &dyn ByteSource) -> Option<[u8; SPARKLINE_BUCKETS]> {
    let h = read_header(src).ok()?;
    let lo = h.min_ele_m as i32;
    let span = h.max_ele_m as i32 - lo;
    if span <= 0 {
        return None;
    }
    let total = h.total_distance_m.max(1) as f64;
    // Peak height per bucket. `i16::MIN` marks a bucket no point landed in.
    let mut peaks = [i16::MIN; SPARKLINE_BUCKETS];
    let mut records = Records::new();
    for k in 0..h.chunk_count {
        let m = stored_meta(src, &h, k).ok()?;
        if m.point_count == 0 {
            return None;
        }
        records.read(src, &m).ok()?;
        for step in walk(m.cum_distance_m, records.points()) {
            let ele = step.p.elevation().filter(|_| !step.p.elevation_incomplete)?;
            let peak = &mut peaks[column(step.along / total, SPARKLINE_BUCKETS)];
            *peak = (*peak).max(ele);
        }
    }
    fill_gaps(&mut peaks, i16::MIN, |p| *p != i16::MIN);
    Some(peaks.map(|p| ((p as i32 - lo) * 255 / span).clamp(0, 255) as u8))
}

/// Make `cols` gap-free: each empty column inherits the nearest filled one, forward first and
/// then backward for a leading run the forward pass cannot reach. A column still empty after both
/// takes `fallback`, so the buffer never keeps a sentinel.
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
