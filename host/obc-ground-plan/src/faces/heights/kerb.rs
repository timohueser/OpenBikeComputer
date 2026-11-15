//! The kerb's rise of a sidewalk point over its host's surface: the class's `kerb_m`, falling with a taper's width
//! to 0 at its tip (`walk::width`), at the point's metres along on the host's mitre-quad frame.

use super::{cross, dot, f32w, norm, sub, Block};
use crate::faces::records::{WALK_LINE, WALK_RULE};
use crate::faces::walk::{width, End};

/// The words of a record line's row in the sidewalks' table, and the rules' first word; None where the block has
/// no table (the far block) or no row for the line (a ceiling line, a storey's line).
fn row(w: &[u32], line: u32) -> Option<(&[u32], usize)> {
    let t = w[15] as usize;
    let n = if t == 0 { 0 } else { w[t + 2] as usize };
    if line as usize >= n {
        return None;
    }
    Some((&w[t + 3 + WALK_LINE * line as usize..][..WALK_LINE], t + 3 + WALK_LINE * n))
}

/// The sidewalk's width, the kerb's rise and the kerb top's width of record line `line`'s class (m); 0 for none.
pub(crate) fn class(w: &[u32], line: u32) -> [f64; 3] {
    row(w, line).map_or([0.0; 3], |(r, _)| [f32w(r[2]), f32w(r[3]), f32w(r[4])])
}

/// q's metres along record line l and its side (+1 on the stroke's left normal), from the segments `segs` of q's
/// bin: in the mitre quad that holds q (the one nearest across where quads overlap) at the share `da / (da + db)`,
/// which is continuous across a mitre line; past the line's first or last sample, along its end chord.
pub(super) fn station(b: &Block, q: [f64; 2], l: u32, segs: &[u32]) -> Option<(f64, f64)> {
    let ln = b.line(l as usize);
    let mut best: Option<(f64, f64, f64)> = None;
    for i in segs.iter().filter(|&&s| s & 0xffff == l).map(|&s| (s >> 16) as usize) {
        let (pa, pb) = (b.position(&ln, i), b.position(&ln, i + 1));
        let (ta, tb) = (b.mitre(&ln, i), b.mitre(&ln, i + 1));
        let ab = sub(pb, pa);
        let len = norm(ab);
        if len <= 0.0 {
            continue;
        }
        let (da, db) = (dot(sub(q, pa), ta), dot(sub(pb, q), tb));
        let (aa, ab_) = (b.along(&ln, i), b.along(&ln, i + 1));
        let along = if da >= 0.0 && db >= 0.0 {
            let t = if da + db > 0.0 { da / (da + db) } else { 0.5 };
            aa + t * (ab_ - aa)
        } else if i == 0 && da < 0.0 && db >= 0.0 {
            aa + (ab_ - aa) * dot(sub(q, pa), ab) / (len * len)
        } else if i + 2 == ln.count && db < 0.0 && da >= 0.0 {
            ab_ + (ab_ - aa) * dot(sub(q, pb), ab) / (len * len)
        } else {
            continue;
        };
        let across = cross(ab, sub(q, pa)) / len;
        if best.is_none_or(|(d, ..)| across.abs() < d) {
            best = Some((across.abs(), along, if across >= 0.0 { 1.0 } else { -1.0 }));
        }
    }
    best.map(|(_, along, side)| (along, side))
}

/// The width rules of record line l on a side.
pub(crate) fn rules(w: &[u32], l: u32, side: f64) -> Vec<End> {
    let Some((r, at)) = row(w, l) else { return Vec::new() };
    let bit = u32::from(side > 0.0);
    (0..r[1] as usize)
        .map(|k| &w[at + WALK_RULE * (r[0] as usize + k)..][..WALK_RULE])
        .filter(|e| e[2] & 1 == bit)
        .map(|e| End { at: f32w(e[0]), dir: if e[2] & 2 != 0 { 1.0 } else { -1.0 }, target: f32w(e[1]) })
        .collect()
}

/// The ramps' slope and round radius of a block's sidewalks.
pub(crate) fn ramp_rules(w: &[u32]) -> (f64, f64) {
    let t = w[15] as usize;
    (f32w(w[t]), f32w(w[t + 1]))
}

/// The kerb's rise at q of a sidewalk on record line `line`: `kerb_m` times the share of the full width that the
/// taper tips on q's side leave at q's metres along. A point that no listed segment frames rises in full.
pub(super) fn rise(b: &Block, q: [f64; 2], line: u32, segs: &[u32]) -> f64 {
    let [full, kerb, _] = class(b.w, line);
    if kerb <= 0.0 || full <= 0.0 {
        return 0.0;
    }
    let Some((at, side)) = station(b, q, line, segs) else { return kerb };
    let (k, r) = ramp_rules(b.w);
    kerb * width(at, &rules(b.w, line, side), full, k, r, true) / full
}

/// The kerb's rise at q (frame units from the cell's origin) of a sidewalk label: 0 for another label, or where q's bin
/// lists no segment.
pub fn rise_at(words: &[u32], q: [f64; 2], label: Option<u32>) -> f64 {
    let b = Block::at(words, q);
    let line = super::material(words, label);
    let sidewalk = label.is_some_and(|id| crate::faces::records::label(words, id)[0] >> 24 == 4);
    match (sidewalk, super::bin_items(&b, q)) {
        (true, Some(items)) if line != u32::MAX => rise(&b, q, line, items.segs),
        _ => 0.0,
    }
}
