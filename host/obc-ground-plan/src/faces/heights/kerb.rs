//! The kerb's rise of a sidewalk point over its host's surface: the class's `kerb_m`, falling with a taper's width
//! to 0 at its tip (`walk::width`), as a function of the point's position.

use super::{f32w, Block};
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
    let m = |r: &[u32]| [r[1] & 0xffff, r[1] >> 16 & 0xff, r[1] >> 24].map(f64::from);
    row(w, line).map_or([0.0; 3], |(r, _)| {
        let [w, kerb, top] = m(r);
        [w / 100.0, kerb / 1000.0, top / 100.0]
    })
}

/// The width rules of record line l, both sides.
pub(crate) fn rules(w: &[u32], l: u32) -> Vec<End> {
    let Some((r, at)) = row(w, l) else { return Vec::new() };
    let snorm = |v: u32| f64::from(v as u16 as i16) / 32767.0;
    (0..(r[0] >> 20) as usize)
        .map(|k| &w[at + WALK_RULE * ((r[0] & 0xf_ffff) as usize + k)..][..WALK_RULE])
        .map(|e| End {
            at: [e[0] as i32, e[1] as i32],
            dir: [snorm(e[2]), snorm(e[2] >> 16)],
            left: e[3] >> 16 & 1 != 0,
            target: f64::from(e[3] & 0xffff) / 100.0,
            across: f64::from(e[3] >> 17) / 100.0,
        })
        .collect()
}

/// The ramps' slope and round radius of a block's sidewalks.
pub(crate) fn ramp_rules(w: &[u32]) -> (f64, f64) {
    let t = w[15] as usize;
    (f32w(w[t]), f32w(w[t + 1]))
}

/// The kerb's rise at q of a sidewalk on record line `line`: `kerb_m` times the share of the full width that the
/// taper tips leave at q.
pub(super) fn rise(b: &Block, q: [f64; 2], line: u32) -> f64 {
    let [full, kerb, _] = class(b.w, line);
    if kerb <= 0.0 || full <= 0.0 {
        return 0.0;
    }
    let (k, r) = ramp_rules(b.w);
    kerb * width(q, &rules(b.w, line), full, k, r, b.mpu, true) / full
}

/// The kerb's rise at q (frame units from the cell's origin) of a sidewalk label: 0 for another label.
pub fn rise_at(words: &[u32], q: [f64; 2], label: Option<u32>) -> f64 {
    let b = Block::at(words, q);
    let line = super::material(words, label);
    let sidewalk = label.is_some_and(|id| crate::faces::records::label(words, id)[0] >> 24 == 4);
    if sidewalk && line != u32::MAX {
        rise(&b, q, line)
    } else {
        0.0
    }
}
