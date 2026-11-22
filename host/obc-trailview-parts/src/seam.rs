//! The face of a seam: where a room goes on as other rooms, one face in the plane of the line, seen from the room
//! before it, from that room's floor up to its outline, with a hole per room behind the line. Its corners are those of
//! the rings that meet it (`Net::seam_cut`), so it shares each edge with a face of a room. A cap, a seam with no room
//! behind it, is dark, as a closed end is; so is a door, where a lane of the room before the line ends on it.

use crate::mesh::{flagged, flat, Builder, Corner, BORE};
use crate::tube::Net;
use crate::Bore;

/// A corner of a region of the face: its metres across the line and up, and its world point.
type At = (Corner, [f64; 3]);

/// The face of seam record r: per part across it, between the walls of the room before the line and the sides of the
/// holes, and over each hole, a polygon of the corners of the seam, cut into triangles.
pub fn face<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, r: usize) {
    let Some(cut) = net.seam_cut(r) else { return };
    let Some((c, _)) = net.opening(net.of_record[r][0]) else { return };
    let inside = &net.rules.inside;
    let colour = match net.bore(&net.chords[c]) {
        _ if cut.holes.is_empty() => inside.void,
        Bore::Road => flagged(inside.tiles, BORE),
        Bore::Track => flagged(inside.lining, BORE),
        Bore::Rock => flagged(inside.rock, BORE),
    };
    let into = net.records[r].way_in();
    let out = flat([-into[0], -into[1]]);
    let (floor, top) = (&cut.floor, &cut.top);
    let mut holes: Vec<(&Vec<At>, bool)> = cut.holes.iter().zip(cut.doors.iter().copied()).collect();
    holes.sort_by(|a, z| a.0[0].0[0].total_cmp(&z.0[0].0[0]));
    let within = |list: &[At], lo: f64, hi: f64| {
        list.iter().copied().filter(|q| q.0[0] >= lo && q.0[0] <= hi).collect::<Vec<At>>()
    };
    let mut regions: Vec<(Vec<At>, u32)> = Vec::new();
    // Between the right wall or a hole's left side and the next hole's right side or the left wall: the floor, up the
    // hole's side, back along the outline, down the side before.
    let mut before: Option<&Vec<At>> = None;
    let mut from = floor[0].0[0];
    for hole in holes.iter().copied().map(Some).chain([None]) {
        let to = hole.map_or(floor[floor.len() - 1].0[0], |h| h.0[0].0[0]);
        let mut poly = within(floor, from, to);
        if let Some((h, _)) = hole {
            poly.extend(h.iter().copied().skip(1).take_while(|q| q.0[0] == to));
        }
        let mut over = within(top, from, to);
        over.reverse();
        poly.extend(over);
        if let Some(h) = before {
            poly.extend(
                h.iter().rev().copied().skip(1).take_while(|q| q.0[0] == from).collect::<Vec<At>>().into_iter().rev(),
            );
        }
        regions.push((poly, colour));
        // Over the hole: its arch from spring to spring, back along the outline. A door is dark: the floor under it,
        // up its side, along its top and down.
        if let Some((h, door)) = hole {
            let (lo, hi) = (h[0].0[0], h[h.len() - 1].0[0]);
            let mut poly: Vec<At> = h[1..h.len() - 1].to_vec();
            let mut over = within(top, lo, hi);
            over.reverse();
            poly.extend(over);
            regions.push((poly, colour));
            if door {
                let mut dark = within(floor, lo, hi);
                dark.extend(h[1..h.len() - 1].iter().rev());
                regions.push((dark, inside.void));
            }
            (before, from) = (Some(h), hi);
        }
    }
    for (mut poly, colour) in regions {
        poly.dedup_by(|a, z| a.0 == z.0);
        while poly.len() > 1 && poly[0].0 == poly[poly.len() - 1].0 {
            poly.pop();
        }
        for [i, j, k] in ear_clip(&poly.iter().map(|q| q.0).collect::<Vec<Corner>>()) {
            let v = [i, j, k].map(|n| b.vertex(poly[n].1, colour, crate::mesh::PLAIN));
            b.triangle(v[0], v[1], v[2], out);
        }
    }
}

/// A new edge of the face passes no other corner nearer than this, where an ear allows it: so no corner lies next to an
/// edge that it is no corner of.
const CLEAR_M: f64 = 0.02;

/// The triangles of a simple polygon by its corners, each corner of the polygon a corner of a triangle beside it: cut
/// off one ear at a time, a corner whose triangle with its two neighbours turns the polygon's way and holds no other
/// corner, not even on its edges, and whose new edge passes no other corner within `CLEAR_M`; of those the ear of the
/// best shape (its area over the squares of its sides).
pub(crate) fn ear_clip(poly: &[Corner]) -> Vec<[usize; 3]> {
    let cross = |a: Corner, b: Corner, c: Corner| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let area: f64 = (0..poly.len()).map(|i| cross([0.0, 0.0], poly[i], poly[(i + 1) % poly.len()])).sum();
    let way = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut left: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    while left.len() > 3 {
        let n = left.len();
        let ear = |i: usize| (left[(i + n - 1) % n], left[i], left[(i + 1) % n]);
        let shape = |i: usize, clear: f64| {
            let (a, b, c) = ear(i);
            let side = |p: Corner, q: Corner| (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
            let twice = way * cross(poly[a], poly[b], poly[c]);
            if twice <= 1e-12 {
                return None;
            }
            // No other corner in the triangle or on its edges.
            let holds = left.iter().any(|&q| {
                q != a && q != b && q != c && poly[q] != poly[a] && poly[q] != poly[b] && poly[q] != poly[c] && {
                    let p = poly[q];
                    let (x, y, z) = (
                        way * cross(poly[a], poly[b], p),
                        way * cross(poly[b], poly[c], p),
                        way * cross(poly[c], poly[a], p),
                    );
                    x >= -1e-12 && y >= -1e-12 && z >= -1e-12
                }
            });
            // The new edge from a to c passes no other corner within `clear`.
            let (d, l2) = ([poly[c][0] - poly[a][0], poly[c][1] - poly[a][1]], side(poly[a], poly[c]));
            let near = left.iter().any(|&q| {
                let p = poly[q];
                let u = ((p[0] - poly[a][0]) * d[0] + (p[1] - poly[a][1]) * d[1]) / l2;
                q != a && q != c && u > 0.0 && u < 1.0 && cross(poly[a], poly[c], p).abs() / l2.sqrt() < clear
            });
            (!holds && !near)
                .then(|| twice / (side(poly[a], poly[b]) + side(poly[b], poly[c]) + side(poly[c], poly[a])))
        };
        let pick =
            |clear: f64| (0..n).filter_map(|i| shape(i, clear).map(|q| (i, q))).max_by(|x, y| x.1.total_cmp(&y.1));
        let best = pick(CLEAR_M).or_else(|| pick(0.0));
        let Some((i, _)) = best else { break };
        let (a, b, c) = ear(i);
        out.push([a, b, c]);
        left.remove(i);
    }
    // The last three corners make a face where they span an area: a region of no width is no face.
    if left.len() == 3 && way * cross(poly[left[0]], poly[left[1]], poly[left[2]]) > 1e-12 {
        out.push([left[0], left[1], left[2]]);
    }
    out
}
