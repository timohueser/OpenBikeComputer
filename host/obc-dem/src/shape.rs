//! GLO-30 made true to the mapped coast, sea cliffs and summits. GLO-30 is a 30 m surface model: it
//! leaves the sea edge above sea level, it spreads a sea cliff over a ramp of up to 100 m on the
//! land, and it rounds a summit into a dome. OSM maps each of these. The shapes use only that
//! evidence and GLO-30 near it:
//!
//! - The water side of `natural=coastline` (the land is on its left) is at sea level, up to
//!   [`CLIFF_REACH_M`] from the line.
//! - A `natural=cliff` within [`CLIFF_REACH_M`] of a coastline is a sea cliff. Near it, each side
//!   takes its own ground, continued in a straight line from [`CLIFF_REACH_M`] and twice that to the
//!   cliff. The higher side only rises, to at most the highest GLO-30 height of its profile, and the
//!   lower side only falls, to at least the lowest. A numeric `height` of the way bounds the drop
//!   across the face. The direction of the way does not matter. Where GLO-30 already shows the face
//!   as steep as [`FACE_SLOPES`], it keeps its heights.
//! - At `natural=peak` or `natural=volcano` with an `ele`, where GLO-30 has its top at the node, the
//!   flanks continued inward from [`PEAK_REACH_M`] and twice that give a cone. Its top is the mean of
//!   the continued flanks. The cone only rises and never above `ele`, so a summit node on a slope, a
//!   wrong `ele` or a dome without a sharper top changes little or nothing.
//!
//! An inland cliff and a coast without a mapped cliff keep their GLO-30 heights. Where GLO-30 is a
//! straight slope across a cliff, the continued ground is GLO-30 itself. The continued ground is
//! read from single GLO-30 samples, so a tree or a roof there can lift a cliff top toward the canopy.
//! A height is a function of the point, the features and GLO-30 alone.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;

use obc_osm::OsmiumRunner;
use serde_json::Value;

/// How far the ramp of a sea cliff in GLO-30 reaches from the mapped line: one and a half posts.
/// Chosen against the LiDAR of the chalk cliffs of East Sussex and of the coast of Dingle: a longer
/// reach sharpens the chalk more, but takes the slope behind a low cliff for its top.
pub const CLIFF_REACH_M: f64 = 45.0;
/// How far GLO-30 rounds a summit: one and a half posts. Chosen against the LiDAR of Glen Coe and
/// Lofoten.
pub const PEAK_REACH_M: f64 = 45.0;
/// How far from its node a summit may stand on the top of GLO-30: one post.
pub const SNAP_M: f64 = 30.0;
/// How far from a point its shaped height reads GLO-30: a summit node within the reach and the
/// snap, a climb past the snap by a quarter post, and the flanks at twice the reach. A sea cliff
/// reads less, three times its reach.
pub const READ_M: f64 = PEAK_REACH_M + 2.0 * SNAP_M + 7.5 + 2.0 * PEAK_REACH_M;
const _: () = assert!(3.0 * CLIFF_REACH_M <= READ_M);
/// The steepest GLO-30 slope across a sea cliff: from the first, GLO-30 starts to hold the face,
/// and from the second it holds it alone. GLO-30 shows the sloping faces of Lofoten steeper than
/// the second, and the blurred chalk cliffs of East Sussex below the first.
pub const FACE_SLOPES: [f64; 2] = [1.0, 2.0];
const M_PER_DEG: f64 = 111_320.0;
/// The side of an index cell in degrees, about 110 m of latitude.
const CELL_DEG: f64 = 1.0 / 1024.0;
/// The directions of the flanks of a summit, as unit vectors east and north.
const H: f64 = std::f64::consts::FRAC_1_SQRT_2;
const FLANKS: [[f64; 2]; 8] = [[1.0, 0.0], [H, H], [0.0, 1.0], [-H, H], [-1.0, 0.0], [-H, -H], [0.0, -1.0], [H, -H]];

/// A mapped line: points as latitude and longitude in degrees.
pub struct Line {
    pub points: Vec<[f64; 2]>,
    /// A coastline, else a cliff.
    pub coast: bool,
    /// The `height` of a cliff in metres: its drop across the face.
    pub height: Option<f64>,
}

/// A mapped summit and its `ele` in metres.
pub struct Peak {
    pub at: [f64; 2],
    pub ele: f64,
}

/// The coastlines, cliffs and summits of an OSM file.
pub type Features = (Vec<Line>, Vec<Peak>);

#[derive(Default)]
struct Cell {
    /// The segments of coastlines and of sea cliffs, each as its line and its first point.
    coast: Vec<(u32, u32)>,
    cliffs: Vec<(u32, u32)>,
    peaks: Vec<u32>,
}

/// The coastlines, sea cliffs and summits within [`CLIFF_REACH_M`] of each cell.
#[derive(Default)]
pub struct Shapes {
    lines: Vec<Line>,
    peaks: Vec<Peak>,
    cells: BTreeMap<(i32, i32), Cell>,
}

fn cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / CELL_DEG).floor() as i32, (lon / CELL_DEG).floor() as i32)
}

/// Each cell that a box of degrees (south, west, north, east), widened by `reach` metres, touches.
fn cells_of([s, w, n, e]: [f64; 4], reach: f64) -> impl Iterator<Item = (i32, i32)> {
    let dlat = reach / M_PER_DEG;
    let dlon = dlat / s.abs().max(n.abs()).min(89.0).to_radians().cos();
    let (lo, hi) = (cell(s - dlat, w - dlon), cell(n + dlat, e + dlon));
    (lo.0..=hi.0).flat_map(move |i| (lo.1..=hi.1).map(move |j| (i, j)))
}

/// The box of degrees (south, west, north, east) of a segment.
fn bounds(a: [f64; 2], b: [f64; 2]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

impl Shapes {
    /// The shapes of `lines` and `peaks`, of the segments and summits that `keep` keeps. Coastlines
    /// that share an end point join, so that the side of a point near a joint is known. A cliff
    /// segment is a sea cliff when a coastline passes within [`CLIFF_REACH_M`] of it.
    pub fn new(lines: Vec<Line>, peaks: Vec<Peak>, keep: impl Fn([f64; 2]) -> bool) -> Self {
        let (coasts, cliffs): (Vec<_>, Vec<_>) = lines.into_iter().partition(|line| line.coast);
        let coasts = join(coasts.into_iter().map(|line| line.points).collect());
        let lines = coasts.into_iter().map(|points| Line { points, coast: true, height: None }).chain(cliffs);
        let lines = lines.filter(|line| line.points.len() > 1).flat_map(|line| {
            let pieces = split(&line.points).into_iter().filter(|points| points.len() > 1);
            pieces.map(move |points| Line { points, ..line })
        });
        let lines: Vec<Line> = lines.collect();
        let peaks: Vec<Peak> = peaks.into_iter().filter(|peak| keep(peak.at)).collect();
        let mut cells = BTreeMap::<(i32, i32), Cell>::new();
        let segments = |coast: bool| {
            let lines = lines.iter().enumerate().filter(move |(_, line)| line.coast == coast);
            lines.flat_map(|(l, line)| {
                (0..line.points.len() - 1).map(move |i| (l, i, line.points[i], line.points[i + 1]))
            })
        };
        for (l, i, a, b) in segments(true).filter(|&(_, _, a, b)| keep(a) || keep(b)) {
            for key in cells_of(bounds(a, b), CLIFF_REACH_M) {
                cells.entry(key).or_default().coast.push((l as u32, i as u32));
            }
        }
        let mut sea_cliffs = Vec::new();
        for (l, i, a, b) in segments(false).filter(|&(_, _, a, b)| keep(a) || keep(b)) {
            let frame = Frame { lat: a[0], lon: a[1], mx: M_PER_DEG * a[0].to_radians().cos() };
            let (fa, fb) = (frame.xy(a), frame.xy(b));
            let near_coast = cells_of(bounds(a, b), 0.0).any(|key| {
                let coast = cells.get(&key).map_or(&[][..], |cell| &cell.coast[..]);
                coast.iter().any(|&(cl, ci)| {
                    let points = &lines[cl as usize].points;
                    let (ca, cb) = (frame.xy(points[ci as usize]), frame.xy(points[ci as usize + 1]));
                    segment_distance([fa, fb], [ca, cb]) < CLIFF_REACH_M
                })
            });
            if near_coast {
                sea_cliffs.push((l, i, a, b));
            }
        }
        for (l, i, a, b) in sea_cliffs {
            for key in cells_of(bounds(a, b), CLIFF_REACH_M) {
                cells.entry(key).or_default().cliffs.push((l as u32, i as u32));
            }
        }
        for (p, peak) in peaks.iter().enumerate() {
            let [lat, lon] = peak.at;
            for key in cells_of([lat, lon, lat, lon], PEAK_REACH_M + SNAP_M) {
                cells.entry(key).or_default().peaks.push(p as u32);
            }
        }
        Self { lines, peaks, cells }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// The shaped height at `lat`, `lon` in degrees, from `glo30`, the GLO-30 height at a point.
    /// It reads GLO-30 up to [`READ_M`] away.
    pub fn height(&self, lat: f64, lon: f64, glo30: &impl Fn(f64, f64) -> Option<f64>) -> Option<f64> {
        let g = glo30(lat, lon)?;
        let Some(cell) = self.cells.get(&cell(lat, lon)) else { return Some(g) };
        let frame = Frame { lat, lon, mx: M_PER_DEG * lat.to_radians().cos() };
        let sample = |at: [f64; 2]| glo30(lat + at[1] / M_PER_DEG, lon + at[0] / frame.mx);
        if self.nearest(&cell.coast, &frame).is_some_and(|near| !self.left(&near, &frame)) {
            return Some(0.0);
        }
        let mut h = g;
        if let Some(near) = self.nearest(&cell.cliffs, &frame) {
            h = near.cliff(g, self.lines[near.line as usize].height, &sample).unwrap_or(g);
        }
        for &p in &cell.peaks {
            if let Some(cone) = cone(&self.peaks[p as usize], &frame, &sample) {
                h = h.max(cone);
            }
        }
        Some(h)
    }

    /// The nearest of `segments` closer than [`CLIFF_REACH_M`].
    fn nearest(&self, segments: &[(u32, u32)], frame: &Frame) -> Option<Near> {
        let mut best: Option<Near> = None;
        for &(l, i) in segments {
            let line = &self.lines[l as usize];
            let (a, b) = (frame.xy(line.points[i as usize]), frame.xy(line.points[i as usize + 1]));
            let v = [b[0] - a[0], b[1] - a[1]];
            let length2 = v[0] * v[0] + v[1] * v[1];
            let t = if length2 > 0.0 { (-(a[0] * v[0] + a[1] * v[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
            let q = [a[0] + t * v[0], a[1] + t * v[1]];
            let d = q[0].hypot(q[1]);
            if d < CLIFF_REACH_M && best.as_ref().is_none_or(|near| d < near.d) {
                let u = match d > 0.0 {
                    true => [-q[0] / d, -q[1] / d],
                    false => {
                        let norm = length2.sqrt().max(f64::MIN_POSITIVE);
                        [-v[1] / norm, v[0] / norm]
                    }
                };
                best = Some(Near { line: l, segment: i, t, d, q, u });
            }
        }
        best
    }

    /// Whether the point of `frame` lies left of the line of `near`. Where the nearest point is a
    /// vertex, the point is left of both segments at a left turn, and of either at a right turn.
    fn left(&self, near: &Near, frame: &Frame) -> bool {
        let line = &self.lines[near.line as usize];
        let count = line.points.len() - 1;
        let closed = line.points[0] == line.points[count];
        let vector = |i: usize| {
            let (a, b) = (frame.xy(line.points[i]), frame.xy(line.points[i + 1]));
            (a, [b[0] - a[0], b[1] - a[1]])
        };
        let left_of = |i: usize| {
            let (a, v) = vector(i);
            v[0] * -a[1] - v[1] * -a[0] > 0.0
        };
        let i = near.segment as usize;
        let joint = if near.t == 0.0 && (i > 0 || closed) {
            Some(((i + count - 1) % count, i))
        } else if near.t == 1.0 && (i + 1 < count || closed) {
            Some((i, (i + 1) % count))
        } else {
            None
        };
        match joint {
            Some((first, second)) => {
                let (u, v) = (vector(first).1, vector(second).1);
                match u[0] * v[1] - u[1] * v[0] > 0.0 {
                    true => left_of(first) && left_of(second),
                    false => left_of(first) || left_of(second),
                }
            }
            None => left_of(i),
        }
    }
}

/// The cone of `peak` at the point of `frame`, never above `ele`. It stands on the top of GLO-30
/// near the node, which a climb in quarter posts finds within [`SNAP_M`]. `None` beyond
/// [`PEAK_REACH_M`] of that top, and where the top is lower than its flanks at a third of the reach
/// or at the reach.
fn cone(peak: &Peak, frame: &Frame, sample: &impl Fn([f64; 2]) -> Option<f64>) -> Option<f64> {
    let mut at = frame.xy(peak.at);
    if at[0].hypot(at[1]) >= PEAK_REACH_M + SNAP_M {
        return None;
    }
    let mut node = sample(at)?;
    for _ in 0..(SNAP_M / 7.5) as usize {
        let mut best = (node, at);
        for u in FLANKS {
            let next = [at[0] + 7.5 * u[0], at[1] + 7.5 * u[1]];
            let height = sample(next)?;
            if height > best.0 {
                best = (height, next);
            }
        }
        if best.1 == at {
            break;
        }
        (node, at) = best;
    }
    let d = at[0].hypot(at[1]);
    if d >= PEAK_REACH_M {
        return None;
    }
    let mut sum = 0.0;
    for u in FLANKS {
        let flank = |s: f64| sample([at[0] + s * u[0], at[1] + s * u[1]]);
        if flank(PEAK_REACH_M / 3.0)? > node || flank(PEAK_REACH_M)? > node {
            return None;
        }
        sum += 2.0 * flank(PEAK_REACH_M)? - flank(2.0 * PEAK_REACH_M)?;
    }
    let top = (sum / FLANKS.len() as f64).min(peak.ele);
    if d == 0.0 {
        return Some(top);
    }
    let rim = sample([at[0] - at[0] / d * PEAK_REACH_M, at[1] - at[1] / d * PEAK_REACH_M])?;
    Some((top + d / PEAK_REACH_M * (rim - top)).min(peak.ele))
}

/// Local metres east and north of a point.
struct Frame {
    lat: f64,
    lon: f64,
    mx: f64,
}

impl Frame {
    fn xy(&self, [lat, lon]: [f64; 2]) -> [f64; 2] {
        [(lon - self.lon) * self.mx, (lat - self.lat) * M_PER_DEG]
    }
}

/// The nearest point of a line: its segment and the place on it, its distance, the point in local
/// metres, and the unit vector from it to the point of the frame.
struct Near {
    line: u32,
    segment: u32,
    t: f64,
    d: f64,
    q: [f64; 2],
    u: [f64; 2],
}

/// The ground of one side of a cliff, continued in a straight line from [`CLIFF_REACH_M`] and twice
/// that, and the range of the side's heights.
struct Side {
    /// At the distance of the point.
    at: f64,
    /// At the cliff.
    edge: f64,
    highest: f64,
    lowest: f64,
}

impl Near {
    /// GLO-30 at `x` metres along the profile, toward the point.
    fn along(&self, x: f64, sample: &impl Fn([f64; 2]) -> Option<f64>) -> Option<f64> {
        sample([self.q[0] + x * self.u[0], self.q[1] + x * self.u[1]])
    }

    /// The side of the point (`sign` 1) or the other side (`sign` −1).
    fn side(&self, sign: f64, sample: &impl Fn([f64; 2]) -> Option<f64>) -> Option<Side> {
        let (mut highest, mut lowest) = (f64::NEG_INFINITY, f64::INFINITY);
        for k in 0..=8 {
            let height = self.along(sign * f64::from(k) * CLIFF_REACH_M / 4.0, sample)?;
            highest = highest.max(height);
            lowest = lowest.min(height);
        }
        let near = self.along(sign * CLIFF_REACH_M, sample)?;
        let slope = (self.along(sign * 2.0 * CLIFF_REACH_M, sample)? - near) / CLIFF_REACH_M;
        Some(Side { at: near + (self.d - CLIFF_REACH_M) * slope, edge: near - CLIFF_REACH_M * slope, highest, lowest })
    }

    /// The height at the point of a sea cliff whose nearest point this is, from `g`, the GLO-30
    /// height there, and the mapped `height` of the cliff.
    fn cliff(&self, g: f64, height: Option<f64>, sample: &impl Fn([f64; 2]) -> Option<f64>) -> Option<f64> {
        let (own, other) = (self.side(1.0, sample)?, self.side(-1.0, sample)?);
        let drop = height.unwrap_or(f64::INFINITY);
        let sharp = match own.edge > other.edge {
            true => g.max(own.at.min(own.highest).min(other.edge + drop)),
            false => g.min(own.at.max(own.lowest).max(other.edge - drop)),
        };
        // The steepest GLO-30 slope across the cliff, in twelve steps of the reach on each side.
        let (step, mut steepest) = (CLIFF_REACH_M / 12.0, 0.0f64);
        let mut last = self.along(-CLIFF_REACH_M, sample)?;
        for k in -11..=12 {
            let height = self.along(f64::from(k) * step, sample)?;
            steepest = steepest.max((height - last).abs() / step);
            last = height;
        }
        let [blurred, held] = FACE_SLOPES;
        let weight = ((held - steepest) / (held - blurred)).clamp(0.0, 1.0);
        Some(g + weight * (sharp - g))
    }
}

/// The distance between two segments in local metres.
fn segment_distance([a, b]: [[f64; 2]; 2], [c, d]: [[f64; 2]; 2]) -> f64 {
    let cross = |o: [f64; 2], p: [f64; 2], q: [f64; 2]| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    if cross(a, b, c) * cross(a, b, d) < 0.0 && cross(c, d, a) * cross(c, d, b) < 0.0 {
        return 0.0;
    }
    let to = |p: [f64; 2], [a, b]: [[f64; 2]; 2]| {
        let v = [b[0] - a[0], b[1] - a[1]];
        let length2 = v[0] * v[0] + v[1] * v[1];
        let t =
            if length2 > 0.0 { (((p[0] - a[0]) * v[0] + (p[1] - a[1]) * v[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
        (p[0] - a[0] - t * v[0]).hypot(p[1] - a[1] - t * v[1])
    };
    to(a, [c, d]).min(to(b, [c, d])).min(to(c, [a, b])).min(to(d, [a, b]))
}

/// Lines that share end points, joined: each line extends forward and backward while another
/// starts at its end or ends at its start. The pieces join once, in order.
fn join(lines: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let key = |point: &[f64; 2]| (point[0].to_bits(), point[1].to_bits());
    let lines: Vec<_> = lines.into_iter().filter(|line| line.len() > 1).collect();
    let mut starts = BTreeMap::new();
    let mut ends = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        starts.entry(key(&line[0])).or_insert(i);
        ends.entry(key(&line[line.len() - 1])).or_insert(i);
    }
    let mut used = vec![false; lines.len()];
    let mut out = Vec::new();
    for i in 0..lines.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut pieces = std::collections::VecDeque::from([i]);
        while let Some(&j) = starts.get(&key(lines[*pieces.back().unwrap()].last().unwrap())).filter(|&&j| !used[j]) {
            used[j] = true;
            pieces.push_back(j);
        }
        while let Some(&j) = ends.get(&key(&lines[pieces[0]][0])).filter(|&&j| !used[j]) {
            used[j] = true;
            pieces.push_front(j);
        }
        let mut line = lines[pieces[0]].clone();
        pieces.iter().skip(1).for_each(|&j| line.extend_from_slice(&lines[j][1..]));
        out.push(line);
    }
    out
}

/// `points` with each segment split into pieces of at most four index cells on each axis, so that
/// a long segment does not reach across many cells. A segment that spans more than 180° of
/// longitude crosses the antimeridian, and the line breaks there.
fn split(points: &[[f64; 2]]) -> Vec<Vec<[f64; 2]>> {
    let mut lines = vec![vec![points[0]]];
    for pair in points.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        if (b[1] - a[1]).abs() > 180.0 {
            lines.push(vec![b]);
            continue;
        }
        let pieces = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) / (4.0 * CELL_DEG)).ceil().max(1.0) as usize;
        let line = lines.last_mut().expect("a line");
        for k in 1..pieces {
            let t = k as f64 / pieces as f64;
            line.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
        }
        line.push(b);
    }
    lines
}

/// How far past the bounds of a bake a feature can change a height: more than the reach of each
/// rule and the GLO-30 it reads.
pub const CLIP_M: f64 = 300.0;

/// The features of the OSM PBF file `pbf` within [`CLIP_M`] of `bounds` (west, south, east, north
/// in degrees), by `osmium`. The filtered files go to `work`.
pub fn read(osmium: &OsmiumRunner, pbf: &Path, bounds: [f64; 4], work: &Path) -> Result<Features, String> {
    let (filtered, clipped) = (work.join("shapes.osm.pbf"), work.join("clipped.osm.pbf"));
    let run = |args: &[&dyn AsRef<std::ffi::OsStr>]| {
        let out =
            osmium.command().args(args).output().map_err(|e| format!("Prepare Osmium, or set OBC_OSMIUM: {e}"))?;
        match out.status.success() {
            true => Ok(()),
            false => Err(format!("Osmium failed: {}", String::from_utf8_lossy(&out.stderr))),
        }
    };
    // `-F pbf`: a store object has no file extension.
    let filters = ["n/natural=peak,volcano", "w/natural=coastline,cliff"];
    run(&[&"tags-filter", &"-F", &"pbf", &"--overwrite", &"-o", &filtered, &pbf, &filters[0], &filters[1]])?;
    let [west, south, east, north] = bounds;
    let dlat = CLIP_M / M_PER_DEG;
    let dlon = dlat / south.abs().max(north.abs()).min(89.0).to_radians().cos();
    let clip = [west - dlon, south - dlat, east + dlon, north + dlat].map(|v| v.to_string()).join(",");
    run(&[&"extract", &"-b", &clip, &"--overwrite", &"-o", &clipped, &filtered])?;
    std::fs::remove_file(&filtered).map_err(|e| format!("{}: {e}", filtered.display()))?;
    let mut child = osmium
        .command()
        .arg("export")
        .arg(&clipped)
        .args(["-f", "geojsonseq", "--geometry-types", "point,linestring", "-x", "print_record_separator=false"])
        .args(["-o", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    let parsed = parse(lines.map(|line| line.map_err(|e| e.to_string())));
    let status = child.wait().map_err(|e| e.to_string())?;
    std::fs::remove_file(&clipped).map_err(|e| format!("{}: {e}", clipped.display()))?;
    match (parsed, status.success()) {
        (Ok(features), true) => Ok(features),
        (Err(error), _) => Err(error),
        (_, false) => Err("Osmium export failed".into()),
    }
}

/// The features of the GeoJSON lines of an Osmium export: `natural=coastline` and `natural=cliff`
/// lines, and `natural=peak` and `natural=volcano` points with an `ele` in metres.
pub fn parse(lines: impl Iterator<Item = Result<String, String>>) -> Result<Features, String> {
    let (mut found, mut peaks) = (Vec::new(), Vec::new());
    for line in lines {
        let line = line?;
        // An export without a feature is one empty line.
        if line.is_empty() {
            continue;
        }
        let feature: Value = serde_json::from_str(&line).map_err(|e| format!("the export of Osmium: {e}"))?;
        let point = |v: &Value| Some([v[1].as_f64()?, v[0].as_f64()?]);
        let (geometry, natural) = (&feature["geometry"], feature["properties"]["natural"].as_str());
        match (geometry["type"].as_str(), natural) {
            (Some("LineString"), Some(kind @ ("coastline" | "cliff"))) => {
                let points = geometry["coordinates"].as_array().into_iter().flatten().map(point).collect();
                let height = feature["properties"]["height"].as_str().and_then(metres);
                if let Some(points) = points {
                    found.push(Line { points, coast: kind == "coastline", height: height.filter(|_| kind == "cliff") });
                }
            }
            (Some("Point"), Some("peak" | "volcano")) => {
                let ele = feature["properties"]["ele"].as_str().and_then(metres);
                if let (Some(at), Some(ele)) = (point(&geometry["coordinates"]), ele) {
                    peaks.push(Peak { at, ele });
                }
            }
            _ => {}
        }
    }
    Ok((found, peaks))
}

/// A tag value in metres, such as `1038.6` or `412 m`.
fn metres(value: &str) -> Option<f64> {
    value.trim().trim_end_matches('m').trim().parse().ok().filter(|metres: &f64| metres.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAT: f64 = 52.0;
    const LON: f64 = -10.0;

    /// The point `x` metres east and `y` metres north of the origin, as latitude and longitude.
    fn at(x: f64, y: f64) -> [f64; 2] {
        [LAT + y / M_PER_DEG, LON + x / (M_PER_DEG * LAT.to_radians().cos())]
    }

    fn line(points: &[(f64, f64)], coast: bool) -> Line {
        Line { points: points.iter().map(|&(x, y)| at(x, y)).collect(), coast, height: None }
    }

    /// The shaped height at `x`, `y` over a GLO-30 that `glo30` gives by the metres east and north.
    fn height(shapes: &Shapes, x: f64, y: f64, glo30: impl Fn(f64, f64) -> f64) -> f64 {
        let metres = |lat: f64, lon: f64| ((lon - LON) * M_PER_DEG * LAT.to_radians().cos(), (lat - LAT) * M_PER_DEG);
        let [lat, lon] = at(x, y);
        shapes.height(lat, lon, &|lat, lon| Some(glo30(metres(lat, lon).0, metres(lat, lon).1))).unwrap()
    }

    /// GLO-30 here reads back through degrees, so it is exact to about a nanometre.
    fn assert_near(height: f64, expected: f64, what: &str) {
        assert!((height - expected).abs() < 1e-6, "{what}: {height} m, not {expected} m");
    }

    /// An island 1 km square, mapped counterclockwise in two ways out of order: the sea around it is
    /// at sea level, also past a corner, and the land and the far sea keep GLO-30.
    #[test]
    fn the_sea_beside_a_coastline_is_at_sea_level() {
        let north = line(&[(1000.0, 1000.0), (0.0, 1000.0), (0.0, 0.0)], true);
        let south = line(&[(0.0, 0.0), (1000.0, 0.0), (1000.0, 1000.0)], true);
        let shapes = Shapes::new(vec![north, south], Vec::new(), |_| true);
        let glo30 = |_: f64, _: f64| 7.0;
        for (x, y) in [(500.0, -20.0), (-20.0, 500.0), (-20.0, -20.0), (1025.0, 1025.0), (500.0, 1040.0)] {
            assert_near(height(&shapes, x, y, glo30), 0.0, &format!("the sea at {x}, {y}"));
        }
        for (x, y) in [(500.0, 20.0), (20.0, 20.0), (980.0, 980.0), (500.0, 500.0), (500.0, -200.0)] {
            assert_near(height(&shapes, x, y, glo30), 7.0, &format!("GLO-30 at {x}, {y}"));
        }
    }

    /// A cliff 30 m high runs north 10 m inland of the coastline. GLO-30 spreads it over a ramp of
    /// 40 m that ends at the coastline; the shapes make it sheer at the mapped line, in either
    /// direction of the way. A `height` bounds the drop. Away from the coast the cliff keeps GLO-30.
    #[test]
    fn a_blurred_sea_cliff_becomes_sheer_in_either_direction() {
        let coast = || line(&[(10.0, -2000.0), (10.0, 2000.0)], true);
        let north = || line(&[(0.0, -2000.0), (0.0, 2000.0)], false);
        let south = line(&[(0.0, 2000.0), (0.0, -2000.0)], false);
        let blurred = |x: f64, _: f64| 30.0 * ((10.0 - x) / 40.0).clamp(0.0, 1.0);
        for cliff in [north(), south] {
            let shapes = Shapes::new(vec![coast(), cliff], Vec::new(), |_| true);
            assert_near(height(&shapes, -3.0, 0.0, blurred), 30.0, "the top at the line");
            assert_near(height(&shapes, -25.0, 0.0, blurred), 30.0, "within the ramp");
            assert_near(height(&shapes, 5.0, 0.0, blurred), 0.0, "the foot");
            assert_near(height(&shapes, -150.0, 0.0, blurred), 30.0, "the plateau");
        }
        let low = Shapes::new(vec![coast(), Line { height: Some(20.0), ..north() }], Vec::new(), |_| true);
        assert_near(height(&low, -3.0, 0.0, blurred), 20.0, "the mapped height");
        let inland = Shapes::new(vec![north()], Vec::new(), |_| true);
        assert_near(height(&inland, -3.0, 0.0, blurred), blurred(-3.0, 0.0), "an inland cliff");
    }

    /// GLO-30 that shows the face at a slope of 1.5 keeps half of the correction, and at a slope
    /// of 3 its own heights.
    #[test]
    fn a_steep_face_in_glo30_fades_the_correction() {
        let shapes = Shapes::new(
            vec![line(&[(10.0, -2000.0), (10.0, 2000.0)], true), line(&[(0.0, -2000.0), (0.0, 2000.0)], false)],
            Vec::new(),
            |_| true,
        );
        let half = |x: f64, _: f64| 30.0 * ((10.0 - x) / 20.0).clamp(0.0, 1.0);
        assert_near(height(&shapes, -3.0, 0.0, half), half(-3.0, 0.0) + 0.5 * (30.0 - half(-3.0, 0.0)), "half");
        let sharp = |x: f64, _: f64| 30.0 * ((10.0 - x) / 10.0).clamp(0.0, 1.0);
        assert_near(height(&shapes, -3.0, 0.0, sharp), sharp(-3.0, 0.0), "GLO-30 already shows the face");
    }

    /// GLO-30 rounds a cone of slope 1 with its top at 1000 m into a dome 20 m lower. A summit node
    /// near the top rises to the top that its flanks show, never above `ele`, and never falls. A
    /// node far down a flank changes nothing.
    #[test]
    fn a_summit_rises_to_its_flanks_only_at_the_top_of_glo30() {
        let dome = |x: f64, y: f64| {
            let r = x.hypot(y);
            if r > 40.0 {
                1000.0 - r
            } else {
                980.0 - r * r / 80.0
            }
        };
        let summit = |x: f64, ele: f64| Shapes::new(Vec::new(), vec![Peak { at: at(x, 0.0), ele }], |_| true);
        let top = |ele: f64| height(&summit(0.0, ele), 0.0, 0.0, dome);
        assert_near(top(2000.0), 1000.0, "the continued flanks");
        assert_near(top(990.0), 990.0, "below the flanks, the ele");
        assert_near(top(950.0), 980.0, "an ele below GLO-30 changes nothing");
        assert_near(height(&summit(0.0, 2000.0), 60.0, 0.0, dome), dome(60.0, 0.0), "beyond the reach");
        assert!(height(&summit(20.0, 2000.0), 0.0, 0.0, dome) > 995.0, "a node beside the top");
        let flank = summit(100.0, 2000.0);
        for x in [0.0, 60.0, 100.0] {
            assert_near(height(&flank, x, 0.0, dome), dome(x, 0.0), "a node down the flank");
        }
        for x in [0.0, 10.0, 30.0] {
            assert!(height(&summit(0.0, 990.0), x, 0.0, dome) <= 990.0_f64.max(dome(x, 0.0)), "above the ele at {x}");
        }
    }

    #[test]
    fn the_export_gives_coastlines_cliffs_and_summits_with_a_height() {
        let lines = [
            r#"{"type":"Feature","geometry":{"type":"LineString","coordinates":[[-9.1,51.0],[-9.0,51.1]]},"properties":{"natural":"coastline"}}"#,
            r#"{"type":"Feature","geometry":{"type":"LineString","coordinates":[[-9.2,51.0],[-9.3,51.1]]},"properties":{"natural":"cliff","height":"35"}}"#,
            r#"{"type":"Feature","geometry":{"type":"LineString","coordinates":[[-9.2,51.0],[-9.3,51.1]]},"properties":{"natural":"ridge"}}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-9.7,52.0]},"properties":{"natural":"peak","ele":"1038.6"}}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-9.6,52.0]},"properties":{"natural":"volcano","ele":"412 m"}}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-9.5,52.0]},"properties":{"natural":"peak","ele":"about 400"}}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-9.5,52.0]},"properties":{"natural":"peak","ele":"1,234"}}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-9.4,52.0]},"properties":{"natural":"peak"}}"#,
        ];
        let (found, peaks) = parse(lines.into_iter().map(|line| Ok(line.to_string()))).unwrap();
        let found: Vec<_> = found.iter().map(|line| (line.coast, line.points[0], line.height)).collect();
        assert_eq!(found, [(true, [51.0, -9.1], None), (false, [51.0, -9.2], Some(35.0))]);
        let peaks: Vec<_> = peaks.iter().map(|peak| (peak.at, peak.ele)).collect();
        assert_eq!(peaks, [([52.0, -9.7], 1038.6), ([52.0, -9.6], 412.0)]);
    }
}
