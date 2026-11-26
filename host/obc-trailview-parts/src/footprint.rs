//! The ring that a part takes from the ground.

use crate::arch::{collar, wide};
use crate::tube::Net;

/// The ring of hole e, in world units, in order round it: its head from front to back (the head wall of all holes of
/// its record, or the run of rims that holds it), and the collar's shell along its chords out to `collar`.
pub fn ring(net: &Net, e: usize) -> Vec<[f64; 2]> {
    let end = &net.holes[e];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let edge = end.half + bake.shell_m;
    let (wide_r, wide_l) = wide(net, e);
    let (front, back) = (net.frame(e, -design.recess_m), net.frame(e, design.face_m - design.recess_m));
    let xy = |p: [f64; 3]| [p[0], p[1]];
    let mut out = vec![xy(front.at([wide_r, 0.0])), xy(front.at([wide_l, 0.0])), xy(back.at([wide_l, 0.0]))];
    let pieces = collar(net, e, end.collar);
    if !pieces.is_empty() {
        let side = |a: f64| pieces.iter().flat_map(move |(s, t, ..)| [xy(s.at([a, 0.0])), xy(t.at([a, 0.0]))]);
        let mut left: Vec<[f64; 2]> = side(edge).collect();
        let mut right: Vec<[f64; 2]> = side(-edge).collect();
        left.dedup();
        right.dedup();
        right.reverse();
        out.extend(left);
        out.extend(right);
    }
    out.push(xy(back.at([wide_r, 0.0])));
    out
}
