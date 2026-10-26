//! The bridges, tunnels and galleries of the TrailView feature tiles: piers, deck depths, the
//! tunnels that run on the ground and the hill side of a gallery. A bridge is a chain of bridge ways
//! that meet end to end. Its piers stand at most SPAN_M apart, never on a road or a path under it and
//! never in water that the bridge crosses for less than CHANNEL_M; its deck is as deep as its longest
//! span asks. A long footbridge high over the ground hangs from cables and has no piers.

use std::collections::HashMap;

use crate::geometry::{arange, bounds, contains, interp, key, Boxes};
use crate::{metres, Tags, Terrain};

/// OSM bridge values of a way that lies on the ground.
const GROUND: [&str; 3] = ["no", "boardwalk", "low_water_crossing"];
const KINDS: [&str; 4] = ["main-road", "road", "path", "railway"];
/// The `railway` values of a track. A tunnel or a gallery of a road can keep another `railway` value.
const TRACKS: [&str; 4] = ["rail", "narrow_gauge", "light_rail", "funicular"];
/// Water crossed for less than CHANNEL_M takes no pier; a pier stands PIER_GAP_M beside it. A pier
/// keeps MIN_SPAN_M from another pier and from the bridge's ends.
const CHANNEL_M: f64 = 30.0;
const PIER_GAP_M: f64 = 2.0;
const MIN_SPAN_M: f64 = 6.0;
/// A pier keeps this far from a waterway's edge and a road's.
const BANK_M: f64 = 1.0;
/// The half-width of a way without a `width` tag, the widest of its class in the engine (PAVED and
/// PATH_HALF of bins.ts).
const MAIN: [&str; 7] = ["motorway", "trunk", "primary", "secondary", "motorway_link", "trunk_link", "primary_link"];
pub const MAIN_HALF_M: f64 = 6.0;
pub const ROAD_HALF_M: f64 = 3.0;
const RAIL_HALF_M: f64 = 1.9;
const PATH_HALF_M: [(&str, f64); 7] = [
    ("path", 1.0),
    ("footway", 1.0),
    ("cycleway", 1.5),
    ("bridleway", 1.0),
    ("steps", 1.0),
    ("pedestrian", 3.0),
    ("via_ferrata", 0.6),
];
/// A footbridge longer than CABLE_M that runs more than HIGH_M over the ground hangs from cables, as
/// one that OSM tags so.
const CABLE_M: f64 = 40.0;
const HIGH_M: f64 = 25.0;
const CABLES: [&str; 3] = ["suspension", "simple-suspension", "cable-stayed"];
const STEP_M: f64 = 1.0;
/// A gallery compares the terrain this far to each side of its way, every GALLERY_STEP_M.
const BESIDE_M: f64 = 10.0;
const GALLERY_STEP_M: f64 = 5.0;
/// The bore of a tunnel for a road, for a path and for a railway with room for its contact wire, and
/// the samples along it.
pub const BORE_M: f64 = 5.0;
pub(crate) const BORE_PATH_M: f64 = 2.6;
pub(crate) const BORE_RAIL_M: f64 = 6.5;
const BORE_STEP_M: f64 = 2.0;
/// Farther than PORTAL_M from its ends, a tunnel that rises FLOAT_M over the terrain lies where the
/// solver could not put it under the hill: a network of shafts and caverns, or a bore that the
/// terrain misses.
const FLOAT_M: f64 = 5.0;
const PORTAL_M: f64 = 20.0;

fn tag<'a>(tags: &'a Tags, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

pub fn is_bridge(kind: &str, tags: &Tags) -> bool {
    KINDS.contains(&kind) && !GROUND.contains(&tag(tags, "bridge").unwrap_or("no"))
}

/// Whether a way is a track: a `railway` of the tracks and no `highway`.
pub fn is_track(tags: &Tags) -> bool {
    tag(tags, "railway").is_some_and(|r| TRACKS.contains(&r)) && !tags.contains_key("highway")
}

/// The class of a way: `rack` for a rack railway, else the `railway` of a track or its `highway`.
pub fn class_of(tags: &Tags) -> &str {
    if is_track(tags) {
        return if tag(tags, "rack").unwrap_or("no") != "no" { "rack" } else { &tags["railway"] };
    }
    tag(tags, "highway").unwrap_or("")
}

/// The height of a tunnel's bore for a way.
pub fn bore(tags: &Tags) -> f64 {
    if is_track(tags) {
        BORE_RAIL_M
    } else if tag(tags, "highway").is_some_and(|h| PATH_HALF_M.iter().any(|(p, _)| *p == h)) {
        BORE_PATH_M
    } else {
        BORE_M
    }
}

/// The half-width of a way in metres: OSM's `width` where it has one, as halfWidth of bins.ts reads
/// it.
pub fn road_half(tags: &Tags) -> f64 {
    if let Some(width) = tag(tags, "width").and_then(leading_number) {
        return (width / 2.0).max(0.4);
    }
    let highway = tag(tags, "highway");
    if is_track(tags) {
        return RAIL_HALF_M;
    }
    if let Some((_, half)) = PATH_HALF_M.iter().find(|(p, _)| Some(*p) == highway) {
        return *half;
    }
    if highway.is_some_and(|h| MAIN.contains(&h)) {
        MAIN_HALF_M
    } else {
        ROAD_HALF_M
    }
}

/// The number at the start of a tag, as JavaScript's parseFloat reads it in the engine: "8 m" is 8.
pub(crate) fn leading_number(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let span = text.find(|c: char| !matches!(c, '0'..='9' | '+' | '-' | '.' | 'e' | 'E')).unwrap_or(text.len());
    (1..=span).rev().find_map(|end| text[..end].parse().ok())
}

/// The bridges: lists of (way, reversed) whose parts meet end to end, where exactly two bridge ends
/// meet.
fn chains(ways: &[&[[f64; 2]]]) -> Vec<Vec<(usize, bool)>> {
    let end_point = |w: usize, end: usize| if end == 0 { ways[w][0] } else { ways[w][ways[w].len() - 1] };
    let mut ends: HashMap<[u64; 2], Vec<(usize, usize)>> = HashMap::new();
    for w in 0..ways.len() {
        for end in [0, 1] {
            ends.entry(key(end_point(w, end))).or_default().push((w, end));
        }
    }
    let neighbour = |w: usize, end: usize| {
        let meeting = &ends[&key(end_point(w, end))];
        if meeting.len() == 2 {
            meeting.iter().copied().find(|&(o, _)| o != w)
        } else {
            None
        }
    };
    let mut seen = vec![false; ways.len()];
    let mut out = Vec::new();
    for start in 0..ways.len() {
        if seen[start] {
            continue;
        }
        // Walk back to the first way of the chain, then forward along it.
        let (mut w, mut end, mut steps) = (start, 0, 0);
        while let Some((o, e)) = neighbour(w, end) {
            if o == start || steps >= ways.len() {
                break;
            }
            (w, end) = (o, 1 - e);
            steps += 1;
        }
        let (mut chain, mut flipped) = (Vec::new(), end == 1);
        while !seen[w] {
            seen[w] = true;
            chain.push((w, flipped));
            let Some((o, e)) = neighbour(w, if flipped { 0 } else { 1 }) else { break };
            (w, flipped) = (o, e == 1);
        }
        out.push(chain);
    }
    out
}

/// Items with the bounding boxes of their points.
struct Boxed<T> {
    boxes: Boxes,
    items: Vec<T>,
}

impl<T> Boxed<T> {
    fn new(items: Vec<T>, points: impl Fn(&T) -> Vec<[f64; 2]>) -> Self {
        let boxes: Vec<[f64; 4]> = items.iter().map(|item| bounds(&points(item))).collect();
        Boxed { boxes: Boxes::new(&boxes), items }
    }

    /// The items whose boxes meet the box from `lo` to `hi`, in order.
    fn near(&self, lo: [f64; 2], hi: [f64; 2]) -> impl Iterator<Item = &T> {
        self.boxes.overlapping([lo[0], lo[1], hi[0], hi[1]]).into_iter().map(|i| &self.items[i])
    }
}

/// The lines of at least 2 points, boxed.
fn lines<'a>(items: &[(f64, &'a [[f64; 2]])]) -> Boxed<(f64, &'a [[f64; 2]])> {
    let items: Vec<(f64, &[[f64; 2]])> = items.iter().copied().filter(|(_, p)| p.len() >= 2).collect();
    Boxed::new(items, |(_, points)| points.to_vec())
}

/// The runs of the samples `xy` within a line's half-width or in an area: (first, last) sample
/// indices.
fn runs(xy: &[[f64; 2]], lines: &Boxed<(f64, &[[f64; 2]])>, areas: &Boxed<&[&[[f64; 2]]]>) -> Vec<(usize, usize)> {
    let mut wet = vec![false; xy.len()];
    let scale: Vec<f64> = xy.iter().map(|p| metres(p[1])).collect();
    let reach = 50.0 / scale.iter().copied().fold(f64::INFINITY, f64::min);
    let [x0, y0, x1, y1] = bounds(xy);
    let (lo, hi) = ([x0 - reach, y0 - reach], [x1 + reach, y1 + reach]);
    for &(half, line) in lines.near(lo, hi) {
        for (k, &[px, py]) in xy.iter().enumerate() {
            let gap = line
                .windows(2)
                .map(|s| {
                    let ([ax, ay], [bx, by]) = (s[0], s[1]);
                    let (dx, dy) = (bx - ax, by - ay);
                    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy).max(1e-30)).clamp(0.0, 1.0);
                    (px - ax - t * dx).hypot(py - ay - t * dy)
                })
                .fold(f64::INFINITY, f64::min);
            wet[k] |= gap * scale[k] <= half + BANK_M;
        }
    }
    for rings in areas.near(lo, hi) {
        for (k, &p) in xy.iter().enumerate() {
            wet[k] |= contains(rings, p);
        }
    }
    let mut out = Vec::new();
    let mut first = None;
    for (k, &w) in wet.iter().chain([&false]).enumerate() {
        match (w, first) {
            (true, None) => first = Some(k),
            (false, Some(f)) => {
                out.push((f, k - 1));
                first = None;
            }
            _ => {}
        }
    }
    out
}

/// The metres along a line at each of its points.
fn along(xy: &[[f64; 2]]) -> Vec<f64> {
    let mut out = vec![0.0];
    for s in xy.windows(2) {
        let ([ax, ay], [bx, by]) = (s[0], s[1]);
        out.push(out[out.len() - 1] + (bx - ax).hypot(by - ay) * metres((ay + by) / 2.0));
    }
    out
}

/// The points along a line at the metres `samples`.
fn sampled(xy: &[[f64; 2]], along: &[f64], samples: &[f64]) -> Vec<[f64; 2]> {
    let (xs, ys): (Vec<f64>, Vec<f64>) = xy.iter().map(|p| (p[0], p[1])).unzip();
    samples.iter().map(|&s| [interp(s, along, &xs), interp(s, along, &ys)]).collect()
}

/// The metres and heights of the points of a line that have one.
fn known(along: &[f64], z: &[f64]) -> (Vec<f64>, Vec<f64>) {
    along.iter().zip(z).filter(|(_, h)| !h.is_nan()).map(|(&s, &h)| (s, h)).unzip()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub bridges: usize,
    pub cable_bridges: usize,
    pub piers: usize,
    pub piers_moved: usize,
}

/// A pier: its point, the height of the deck there and a way of its bridge.
pub type Pier = ([f64; 2], f64, usize);

/// A bridge way: its kind, tags, points and heights (NaN where unknown).
pub type Way<'a> = (&'a str, &'a Tags, &'a [[f64; 2]], &'a [f64]);

/// Per bridge way, the tags that the bake adds: its deck depth in metres, and `bridge:structure` for
/// a cable bridge; and the piers as (point, deck height there, a way of their bridge). `waterways` and
/// `roads` are (half-width, points), `areas` the rings of river areas and lakes.
pub fn bridges(
    ways: &[Way],
    waterways: &[(f64, &[[f64; 2]])],
    areas: &[Vec<&[[f64; 2]]>],
    terrain: &dyn Terrain,
    roads: &[(f64, &[[f64; 2]])],
) -> (Vec<Option<Tags>>, Vec<Pier>, Metrics) {
    let (waterways, roads) = (lines(waterways), lines(roads));
    let areas = Boxed::new(areas.iter().map(Vec::as_slice).collect(), |rings| rings.concat());
    let dry: Boxed<&[&[[f64; 2]]]> = Boxed::new(Vec::new(), |_| Vec::new());
    let (mut added, mut piers, mut metrics) = (vec![None; ways.len()], Vec::new(), Metrics::default());
    let points: Vec<&[[f64; 2]]> = ways.iter().map(|w| w.2).collect();
    for chain in chains(&points) {
        let (mut xy, mut z): (Vec<[f64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
        for &(w, flipped) in &chain {
            let (mut p, mut h) = (ways[w].2.to_vec(), ways[w].3.to_vec());
            if flipped {
                p.reverse();
                h.reverse();
            }
            let skip = usize::from(!xy.is_empty());
            xy.extend_from_slice(&p[skip..]);
            z.extend_from_slice(&h[usize::from(!z.is_empty())..]);
        }
        let along = along(&xy);
        let total = along[along.len() - 1];
        let path = chain.iter().all(|&(w, _)| ways[w].0 == "path");
        let (along_known, z_known) = known(&along, &z);
        let samples = arange(0.0, total + STEP_M / 2.0, STEP_M);
        let at = sampled(&xy, &along, &samples);
        let tagged = chain.iter().filter_map(|&(w, _)| tag(ways[w].1, "bridge:structure")).find(|s| CABLES.contains(s));
        let mut structure = tagged.map(str::to_string);
        if structure.is_none() && path && total > CABLE_M && along_known.len() >= 2 {
            let ground = terrain.heights(&at);
            let high = samples
                .iter()
                .zip(&ground)
                .map(|(&s, &g)| interp(s, &along_known, &z_known) - g)
                .filter(|h| !h.is_nan())
                .fold(0.0, f64::max);
            structure = (high > HIGH_M).then(|| "simple-suspension".to_string());
        }
        metrics.cable_bridges += usize::from(structure.is_some());
        let span_m = if path { 30.0 } else { 45.0 };
        let spans = if structure.is_some() { 1 } else { ((total / span_m).ceil() as usize).max(1) };
        let mut stations: Vec<f64> = (1..spans).map(|k| k as f64 * total / spans as f64).collect();
        if !stations.is_empty() {
            let wet = runs(&at, &waterways, &areas).into_iter().map(|(f, l)| (samples[f], samples[l]));
            let blocked = wet.filter(|(lo, hi)| hi - lo < CHANNEL_M);
            let under = runs(&at, &roads, &dry).into_iter().map(|(f, l)| (samples[f], samples[l]));
            for (lo, hi) in blocked.chain(under) {
                for s in stations.iter_mut() {
                    if lo - PIER_GAP_M < *s && *s < hi + PIER_GAP_M {
                        *s = if *s - lo < hi - *s { lo - PIER_GAP_M } else { hi + PIER_GAP_M };
                        metrics.piers_moved += 1;
                    }
                }
            }
            stations.sort_by(f64::total_cmp);
            let mut kept: Vec<f64> = Vec::new();
            for s in stations {
                if (MIN_SPAN_M..=total - MIN_SPAN_M).contains(&s) && kept.last().is_none_or(|&k| s - k >= MIN_SPAN_M) {
                    kept.push(s);
                }
            }
            stations = kept;
        }
        let (rate, least, most): (f64, f64, f64) = if path { (1.0 / 40.0, 0.3, 1.0) } else { (1.0 / 30.0, 0.5, 2.5) };
        let stops: Vec<f64> = [0.0].into_iter().chain(stations.iter().copied()).chain([total]).collect();
        let longest = stops.windows(2).map(|s| s[1] - s[0]).fold(f64::NEG_INFINITY, f64::max);
        // To the decimetre, half to even after the product with 10.
        let depth = if structure.is_some() {
            least
        } else {
            (most.min(least.max(longest * rate)) * 10.0).round_ties_even() / 10.0
        };
        for &(w, _) in &chain {
            let mut tags = Tags::from([("deck".to_string(), format!("{depth:.1}"))]);
            if let Some(structure) = &structure {
                tags.insert("bridge:structure".into(), structure.clone());
            }
            added[w] = Some(tags);
        }
        if along_known.len() < 2 {
            continue;
        }
        let (xs, ys): (Vec<f64>, Vec<f64>) = xy.iter().map(|p| (p[0], p[1])).unzip();
        for s in stations {
            let point = [interp(s, &along, &xs), interp(s, &along, &ys)];
            piers.push((point, interp(s, &along_known, &z_known), chain[0].0));
        }
    }
    metrics.bridges = added.iter().filter(|a| a.is_some()).count();
    metrics.piers = piers.len();
    (added, piers, metrics)
}

/// The side of a gallery that the hill rises on, "left" or "right" of its way, from the terrain
/// BESIDE_M to each side of it.
pub fn uphill(points: &[[f64; 2]], terrain: &dyn Terrain) -> &'static str {
    let (mut ahead, mut behind) = (Vec::new(), Vec::new());
    for s in points.windows(2) {
        let ([ax, ay], [bx, by]) = (s[0], s[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let length = dx.hypot(dy);
        let steps = ((length * metres(ay) / GALLERY_STEP_M).ceil() as usize).max(1);
        let (lx, ly) = (dy / length.max(1e-15), -dx / length.max(1e-15));
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let (x, y) = (ax + t * dx, ay + t * dy);
            let reach = BESIDE_M / metres(y);
            ahead.push([x + lx * reach, y + ly * reach]);
            behind.push([x - lx * reach, y - ly * reach]);
        }
    }
    // Only the sign of the summed rise counts; an unknown one adds nothing.
    let rise: Vec<f64> = terrain.heights(&ahead).iter().zip(terrain.heights(&behind)).map(|(a, b)| a - b).collect();
    let known = rise.iter().any(|d| !d.is_nan());
    if known && rise.iter().filter(|d| !d.is_nan()).sum::<f64>() < 0.0 {
        "right"
    } else {
        "left"
    }
}

/// The terrain over a tunnel at samples along it, in metres over its road, and the samples' metres
/// from its nearer end; None with fewer than two heights.
fn cover(points: &[[f64; 2]], z: &[f64], terrain: &dyn Terrain) -> Option<(Vec<f64>, Vec<f64>)> {
    let along = along(points);
    let (along_known, z_known) = known(&along, z);
    if along_known.len() < 2 {
        return None;
    }
    let total = along[along.len() - 1];
    let samples = arange(0.0, total + BORE_STEP_M / 2.0, BORE_STEP_M);
    let ground = terrain.heights(&sampled(points, &along, &samples));
    let depth = samples.iter().zip(&ground).map(|(&s, &g)| g - interp(s, &along_known, &z_known)).collect();
    Some((depth, samples.iter().map(|&s| s.min(total - s)).collect()))
}

/// Whether a tunnel has less than its bore of terrain over it everywhere: what covers it (a building,
/// a deck) is not in the terrain, so it runs on the ground.
pub fn shallow(tags: &Tags, points: &[[f64; 2]], z: &[f64], terrain: &dyn Terrain) -> bool {
    cover(points, z, terrain).is_some_and(|(depth, _)| {
        depth.iter().filter(|d| !d.is_nan()).fold(f64::NEG_INFINITY, |m, &d| m.max(d)) < bore(tags)
    })
}

/// Whether a tunnel rises more than FLOAT_M over the terrain farther than PORTAL_M from its ends.
pub fn floating(points: &[[f64; 2]], z: &[f64], terrain: &dyn Terrain) -> bool {
    cover(points, z, terrain).is_some_and(|(depth, end)| {
        let inner = depth.iter().zip(&end).filter(|(d, &e)| e > PORTAL_M && !d.is_nan());
        inner.fold(f64::INFINITY, |m, (&d, _)| m.min(d)) < -FLOAT_M
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metres east and north of a point at about 46.7° N, in world units.
    const Y: f64 = 0.3218;

    fn at(east: f64, north: f64) -> [f64; 2] {
        let m = 1.0 / metres(Y);
        [0.53 + east * m, Y - north * m]
    }

    fn east(point: [f64; 2]) -> f64 {
        (point[0] - 0.53) * metres(Y)
    }

    fn flat(_: f64, _: f64) -> f64 {
        500.0
    }

    #[test]
    fn a_tunnel_under_a_mountain_does_not_float_and_over_a_valley_it_does() {
        let mountain = |x: f64, _: f64| if (x - 0.53).abs() < 2995.0 / metres(Y) { 2500.0 } else { 1000.0 };
        let points = [at(-3000.0, 0.0), at(-1000.0, 0.0), at(0.0, 0.0), at(3000.0, 0.0)];
        let z = [1000.0; 4];
        assert!(!floating(&points, &z, &mountain));
        assert!(floating(&points, &z, &flat));
    }

    #[test]
    fn a_pier_steps_out_of_a_narrow_channel_and_a_high_footbridge_hangs() {
        let none = Tags::new();
        let (road, path) = ([at(0.0, 0.0), at(80.0, 0.0)], [at(0.0, 50.0), at(120.0, 50.0)]);
        let ways: [Way; 2] = [("road", &none, &road, &[500.0, 500.0]), ("path", &none, &path, &[600.0, 600.0])];
        let channel = [at(40.0, -50.0), at(40.0, 100.0)];
        let (added, piers, metrics) = bridges(&ways, &[(5.0, &channel)], &[], &flat, &[]);
        assert_eq!(piers.len(), 1);
        assert!((east(piers[0].0) - 40.0).abs() > 5.0 + BANK_M);
        assert_eq!(added[1].as_ref().unwrap()["bridge:structure"], "simple-suspension");
        assert_eq!(metrics.cable_bridges, 1);
    }

    #[test]
    fn a_pier_stands_beside_the_road_under_its_bridge() {
        let none = Tags::new();
        let deck = [at(0.0, 0.0), at(80.0, 0.0)];
        let road = [at(40.0, -50.0), at(40.0, 50.0)];
        let (_, piers, _) =
            bridges(&[("road", &none, &deck, &[510.0, 510.0])], &[], &[], &flat, &[(ROAD_HALF_M, &road)]);
        assert!((east(piers[0].0) - 40.0).abs() > ROAD_HALF_M + BANK_M);
    }

    #[test]
    fn a_width_reads_as_a_number_at_the_start_of_its_tag() {
        let width = |w: &str| road_half(&Tags::from([("width".into(), w.into())]));
        assert_eq!(
            (width(" 8 m"), width("3."), width(".5"), width("-3"), width("wide")),
            (4.0, 1.5, 0.4, 0.4, ROAD_HALF_M)
        );
        let n = leading_number;
        assert_eq!((n(" -2.5e1x"), n("1e"), n("1e+"), n("5-3")), (Some(-25.0), Some(1.0), Some(1.0), Some(5.0)));
        assert_eq!((n("m8"), n("."), n(""), n("+-5"), n("inf")), (None, None, None, None, None));
    }
}
