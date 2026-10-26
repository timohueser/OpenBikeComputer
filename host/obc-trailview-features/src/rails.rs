//! The railway furniture of the TrailView feature tiles: the masts of the contact wire of the
//! electrified tracks, and the heights of the platforms. Masts stand evenly along each track, at most
//! MAST_M apart, on the side away from a neighbouring track, and none within MAST_GAP_M of another; a
//! track with neighbours on both sides has no masts, as the arms of a mast reach only over its own
//! track. A platform's top lies PLATFORM_M over the rail of the track nearest to each of its vertices.

use std::collections::BTreeSet;

use rstar::primitives::GeomWithData;
use rstar::RTree;

use crate::{metres, Part, Tags};

/// The most metres between two masts of a track, and how far beside the track's middle a mast
/// stands, for a standard and for a narrow gauge. On a deck a mast stands against the deck's edge: the
/// half-width of the track's bed in the engine (SURFACES of bins.ts) and half the mast's side. A mast
/// within MAST_GAP_M of one placed before it, as at a switch, is left out.
const MAST_M: f64 = 50.0;
const MAST_GAP_M: f64 = 8.0;
const MAST_SIDE_M: [f64; 2] = [3.0, 2.5];
const DECK_SIDE_M: [f64; 2] = [1.9 + 0.12, 1.5 + 0.12];
/// A track within NEIGHBOUR_M beside a mast, but farther than NEAR_M from its track, is a neighbour.
const NEIGHBOUR_M: f64 = 8.0;
const NEAR_M: f64 = 1.0;
const SAMPLE_M: f64 = 2.0;
/// A platform's top over the rail, for a standard and for a narrow gauge; a vertex reads the nearest
/// track within PLATFORM_REACH_M.
const PLATFORM_M: [f64; 2] = [0.55, 0.35];
const PLATFORM_REACH_M: f64 = 15.0;
const NARROW_MM: u32 = 1200;

/// Whether this module sets the heights of a feature.
pub fn solves(kind: &str) -> bool {
    matches!(kind, "platform" | "platform-line")
}

/// Whether a track is narrower than standard gauge: by the leading digits of its first `gauge` in mm,
/// as `trackSurface` of bins.ts reads it, else by its class.
fn narrow(tags: &Tags) -> bool {
    let gauge = tags.get("gauge").map_or("", String::as_str).split(';').next().unwrap_or("");
    let digits = gauge.trim_start();
    let digits = &digits[..digits.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return matches!(tags.get("railway").map(String::as_str), Some("narrow_gauge" | "funicular"));
    }
    let digits = digits.trim_start_matches('0');
    digits.len() <= 4 && digits.parse::<u32>().unwrap_or(0) < NARROW_MM
}

/// Whether a track runs under a contact wire.
pub fn electrified(tags: &Tags) -> bool {
    matches!(tags.get("electrified").map(String::as_str), Some("contact_line" | "yes"))
        && tags.get("railway").map(String::as_str) != Some("funicular")
}

/// A track: its tags, points and heights (NaN where unknown).
pub type Track<'a> = (&'a Tags, &'a [[f64; 2]], &'a [f64]);

/// Points every SAMPLE_M along the tracks, with their heights and their tracks.
fn samples(tracks: &[Track]) -> (Vec<[f64; 2]>, Vec<f64>, Vec<usize>) {
    let (mut xy, mut z, mut owner) = (Vec::new(), Vec::new(), Vec::new());
    for (t, &(_, points, heights)) in tracks.iter().enumerate() {
        for k in 1..points.len() {
            let ([ax, ay], [bx, by]) = (points[k - 1], points[k]);
            let (ha, hb) = (heights[k - 1], heights[k]);
            let n = (((bx - ax).hypot(by - ay) * metres(ay) / SAMPLE_M).ceil() as usize).max(1);
            for i in 0..n {
                let u = i as f64 / n as f64;
                xy.push([ax + u * (bx - ax), ay + u * (by - ay)]);
                z.push(ha + u * (hb - ha));
                owner.push(t);
            }
        }
        if let (Some(&p), Some(&h)) = (points.last(), heights.last()) {
            xy.push(p);
            z.push(h);
            owner.push(t);
        }
    }
    (xy, z, owner)
}

fn tree(xy: &[[f64; 2]]) -> RTree<GeomWithData<[f64; 2], usize>> {
    RTree::bulk_load(xy.iter().enumerate().map(|(i, &p)| GeomWithData::new(p, i)).collect())
}

/// The masts: (point, the height of its track's rail, tags) per mast. `tracks` are the electrified
/// tracks on the ground and on bridges, with whether each lies on a deck; `beside` every track out of
/// a tunnel, the neighbours.
pub fn masts(tracks: &[(Track, bool)], beside: &[Track]) -> Vec<([f64; 2], f64, Tags)> {
    let (xy, _, _) = samples(beside);
    let neighbours = tree(&xy);
    let mut out: Vec<([f64; 2], f64, Tags)> = Vec::new();
    for &((tags, p, h), deck) in tracks {
        let lengths: Vec<f64> = p
            .windows(2)
            .map(|s| (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1]) * metres((s[0][1] + s[1][1]) / 2.0))
            .collect();
        let mut along = vec![0.0];
        for l in &lengths {
            along.push(along[along.len() - 1] + l);
        }
        let total = along[along.len() - 1];
        if total <= 0.0 {
            continue;
        }
        // A centimetre over the spacing takes no extra mast.
        let n = (((total - 0.01) / MAST_M).ceil() as usize).max(1);
        let side_m = (if deck { DECK_SIDE_M } else { MAST_SIDE_M })[usize::from(narrow(tags))];
        for i in 0..n {
            let s = (i as f64 + 0.5) * total / n as f64;
            let k = (along.partition_point(|&a| a <= s) - 1).min(lengths.len() - 1);
            let u = (s - along[k]) / lengths[k].max(1e-9);
            let (d, height) = ([p[k + 1][0] - p[k][0], p[k + 1][1] - p[k][1]], h[k] + u * (h[k + 1] - h[k]));
            let at = [p[k][0] + u * d[0], p[k][1] + u * d[1]];
            if !height.is_finite() {
                continue;
            }
            let norm = d[0].hypot(d[1]).max(1e-300);
            let left = [d[1] / norm, -d[0] / norm];
            let unit = metres(at[1]);
            let reach = NEIGHBOUR_M / unit;
            let mut sides = BTreeSet::new();
            for near in neighbours.locate_within_distance(at, reach * reach) {
                let [ox, oy] = [near.geom()[0] - at[0], near.geom()[1] - at[1]];
                let offset = (ox * left[0] + oy * left[1]) * unit;
                if offset.abs() > NEAR_M && ox.hypot(oy) * unit > NEAR_M {
                    sides.insert(offset > 0.0);
                }
            }
            if sides.len() == 2 {
                continue;
            }
            let sign = if sides.contains(&true) { -1.0 } else { 1.0 };
            let point = [at[0] + sign * left[0] * side_m / unit, at[1] + sign * left[1] * side_m / unit];
            let tags = if deck { Tags::from([("on".into(), "deck".into())]) } else { Tags::new() };
            out.push((point, height, tags));
        }
    }
    // A mast within MAST_GAP_M of a mast kept before it goes.
    let placed = tree(&out.iter().map(|m| m.0).collect::<Vec<_>>());
    let mut kept = vec![false; out.len()];
    for i in 0..out.len() {
        let reach = MAST_GAP_M / metres(out[i].0[1]);
        kept[i] = !placed.locate_within_distance(out[i].0, reach * reach).any(|m| m.data < i && kept[m.data]);
    }
    out.into_iter().zip(kept).filter(|(_, keep)| *keep).map(|(mast, _)| mast).collect()
}

/// Per platform, its parts with a height on each vertex: PLATFORM_M over the rail of the nearest
/// track within PLATFORM_REACH_M, or the middle height of its other vertices; None where no vertex has
/// a track near.
pub fn platforms(platforms: &[Vec<Part>], tracks: &[Track]) -> Vec<Option<Vec<Part>>> {
    let (xy, z, owner) = samples(tracks);
    let known: Vec<usize> = (0..z.len()).filter(|&i| z[i].is_finite()).collect();
    let rails = tree(&known.iter().map(|&i| xy[i]).collect::<Vec<_>>());
    let lift: Vec<f64> = tracks.iter().map(|(tags, _, _)| PLATFORM_M[usize::from(narrow(tags))]).collect();
    let mut out = Vec::new();
    for platform in platforms {
        let mut rows = Vec::new();
        for part in platform {
            let mut heights = vec![f64::NAN; part.points.len()];
            if !known.is_empty() {
                let lowest = part.points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
                let bound = PLATFORM_REACH_M / metres(lowest);
                for (height, &p) in heights.iter_mut().zip(&part.points) {
                    // Of equally near samples, the first.
                    let Some(near) = rails.nearest_neighbors(&p).into_iter().min_by_key(|n| n.data) else { continue };
                    let [dx, dy] = [near.geom()[0] - p[0], near.geom()[1] - p[1]];
                    let d2 = dx * dx + dy * dy;
                    if d2 < bound * bound && d2.sqrt() * metres(p[1]) <= PLATFORM_REACH_M {
                        let i = known[near.data];
                        *height = z[i] + lift[owner[i]];
                    }
                }
            }
            rows.push(heights);
        }
        let mut found: Vec<f64> = rows.iter().flatten().copied().filter(|h| h.is_finite()).collect();
        if found.is_empty() {
            out.push(None);
            continue;
        }
        found.sort_by(f64::total_cmp);
        let m = found.len();
        let middle = if m % 2 == 1 { found[m / 2] } else { (found[m / 2 - 1] + found[m / 2]) / 2.0 };
        let parts = platform
            .iter()
            .zip(rows)
            .map(|(part, heights)| Part {
                outer: part.outer,
                points: part.points.clone(),
                z: Some(heights.into_iter().map(|h| if h.is_finite() { h } else { middle }).collect()),
            })
            .collect();
        out.push(Some(parts));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structures::{bore, class_of, BORE_M};

    /// A point at Engelberg in world units, and world units per metre there.
    fn origin() -> (f64, f64, f64) {
        let y0 = (1.0 - 46.82f64.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;
        ((8.40 + 180.0) / 360.0, y0, 1.0 / metres(y0))
    }

    fn at(east: f64, north: f64) -> [f64; 2] {
        let (x0, y0, unit) = origin();
        [x0 + east * unit, y0 - north * unit]
    }

    fn rail() -> Tags {
        Tags::from([("railway".into(), "rail".into())])
    }

    #[test]
    fn a_road_tunnel_with_an_old_railway_tag_stays_a_road() {
        let tunnel: Tags = [("highway", "tertiary"), ("tunnel", "yes"), ("railway", "abandoned")]
            .map(|(k, v)| (k.into(), v.into()))
            .into();
        assert_eq!((class_of(&tunnel), bore(&tunnel)), ("tertiary", BORE_M));
    }

    #[test]
    fn masts_stand_outside_a_double_track_and_a_platform_stands_over_its_rail() {
        let tags = rail();
        let line = |north: f64| vec![at(0.0, north), at(200.0, north)];
        let (north, south, middle) = (line(4.5), line(0.0), line(-4.5));
        let flat = [500.0, 500.0];
        let tracks: Vec<Track> = [&north, &south, &middle].map(|p| (&tags, p.as_slice(), &flat[..])).into();
        let wired: Vec<(Track, bool)> = tracks.iter().map(|&t| (t, false)).collect();
        let found = masts(&wired, &tracks);
        let (_, y0, unit) = origin();
        let offsets: BTreeSet<i64> = found.iter().map(|(p, _, _)| ((y0 - p[1]) / unit * 10.0).round() as i64).collect();
        // The north track's masts stand north of it, the south track's south; the middle track has none.
        let side = MAST_SIDE_M[0];
        assert_eq!(offsets, BTreeSet::from([(-(4.5 + side) * 10.0) as i64, ((4.5 + side) * 10.0) as i64]));
        assert_eq!(found.len(), 2 * 4);
        let (a, b) = (vec![at(0.0, 50.0), at(4.0, 50.0)], vec![at(4.0, 50.0), at(8.0, 50.0)]);
        let switch = [((&tags, a.as_slice(), &flat[..]), false), ((&tags, b.as_slice(), &flat[..]), false)];
        assert_eq!(masts(&switch, &[]).len(), 1);
        let platform = vec![Part {
            outer: true,
            points: vec![at(20.0, 7.0), at(180.0, 7.0), at(180.0, 13.0), at(20.0, 13.0)],
            z: None,
        }];
        let solved = platforms(&[platform], &tracks[..1]);
        assert_eq!(solved[0].as_ref().unwrap()[0].z, Some(vec![500.0 + PLATFORM_M[0]; 4]));
        let far = vec![Part { outer: true, points: vec![at(0.0, 500.0); 3], z: None }];
        assert_eq!(platforms(&[far], &tracks[..1]), vec![None]);
    }
}
