//! The far block of a cell: the record lines of its records' lines that reach it, each with a run of
//! its samples, for the pages that draw the ways from their lines alone. A full build encodes it from
//! the lines of its candidates, a far-only build (`far_block`) from the same lines without them: one
//! encoder of one set of lines, so both give the same words.
//!
//! Little-endian words; coordinates are frame units from the cell's origin. Words 3, 6, 7 and 10 of
//! the head, the rules, the lines and the samples sit where the near block's do, so a reader of those
//! reads both blocks. Words 8 and 9 are not the near block's extras and bins, so a reader of bins or
//! candidates never reads a far block. A change of the layout changes `MAGIC_FAR`.
//!
//! | Words | Holds |
//! | --- | --- |
//! | 0 | `MAGIC_FAR` |
//! | 1, 2 | 0: no candidates, no labels |
//! | 3 | the count of record lines L |
//! | 4, 5, 6 | the offset of the lines: `HEAD` + `heights::RULES` |
//! | 7, 8, 9 | the offsets of the samples, the keys and the sides |
//! | 10 | the cell's row of zoom 16 |
//! | 11, 12, 14 | the length of the block: the near block's blend edges, junctions and outline edges are empty; 13 is 0 |
//! | rules | `heights::RULES` words, as the near block's |
//! | lines | `LINE` words per record line, as the near block's, but word 2 is its first sample in this block and word 3 its count of samples here |
//! | samples | `SAMPLE` words each, as the near block's, but word 4 is the rank (0 to n − 1, below 2^20) of the sample's global name among the n names of this block's samples |
//! | keys | a word per record line: the key of its way's carriageway or rail bed candidate, rank << 16, class << 8, kind; its numeric order is the overlay's order of keys |
//! | sides | 2 words per sample: the plain terrain (f32 bits) where the cones of its left and of its right side let go; unknown is the NaN 0x7fc0_0000 |
//!
//! A line is in the block when one of its segments reaches the cell: the segment's box grown by the
//! line's `reach` meets the cell square. Then each of its legs is a record line, in the near block's
//! order. A leg with no segment in reach holds no samples. Else its run ends at its leg's end, or
//! `PAD` segments before its first segment in reach and `PAD` + 1 samples after its last. The line
//! words are the whole leg's, so at a run end that is not its leg's end the end kinds (line words 12
//! and 13) and `h_segment`'s first and last segment are not true: a reader reads there only what the
//! stroke of a segment in reach reads, the segments within `WINDOW` of it and their neighbours.
//!
//! A side's point is the sample moved along the left normal (m.y, −m.x) of its mitre m by ± (its
//! shoulder's outer offset + the class's slope reach × (1 − `fade_share`)), rounded to whole units:
//! where the cones of an outline edge let go (`outline_edges`).

use super::{f, f32w, key_word, line_words, outline_units, HEAD, LEG_SHIFT, SAMPLE};
use crate::faces::geom::P;
use crate::faces::{legs, record_lines, way_key, Glob, Rules, WayLine};
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::graph::WayGraph;
use crate::CellInput;

/// "OBF" and the layout's version.
pub const MAGIC_FAR: u32 = 0x4f42_4601;
/// The viewer's road mask reaches this far past a way's core (`MASK_M` of `terrain/features.wgsl`).
pub const MASK_M: f64 = 12.75;
/// The segments on each side of its own that the stroke of a segment reads.
pub const WINDOW: usize = 4;
/// The segments of a run before its first segment in reach: a stroke's window, and the neighbour on
/// each side that `h_segment` reads.
pub const PAD: usize = WINDOW + 1;
/// The most samples of a run: the viewer packs a sample's index in 16 bits.
pub const RUN_MAX: usize = 65_535;
/// More names than this in one block are an error: the viewer packs a sample's rank in 20 bits.
pub const NAMES_MAX: usize = 1 << 20;

/// The far block of a cell's ground level alone: the lines of its records, without candidates.
pub fn far_block(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey) -> Result<Vec<u32>, String> {
    encode(cell, graph, input, rules, &record_lines(graph, input, rules, cell))
}

/// A line's reach past a segment, in units at the cell's edge nearer the pole: its widest carriageway,
/// then its core and slope or the mask's reach, whichever is farther, and a metre. It holds for the
/// pages of level 16 and finer, where two texels are less than the mask's reach.
fn reach(cell: CellKey, line: &WayLine) -> f64 {
    let y = cell.origin().y;
    let half = (line.p.iter().zip(&line.h)).map(|(p, h)| h * metres_per_unit(y + i64::from(p[1]))).fold(0.0, f64::max);
    let s = &line.sec;
    outline_units(cell, half + (s.shoulder_m + s.verge_m + s.reach_m).max(MASK_M) + 1.0)
}

/// Per leg [lo, hi] its run of samples, none where no segment of it is in reach r of the cell square.
/// Segment i joins the samples i and i + 1.
fn runs(p: &[P], legs: &[(usize, usize)], r: f64) -> Vec<Option<(usize, usize)>> {
    let cell = f64::from(CELL_UNITS as i32);
    let meets = |i: &usize| {
        let (a, b) = (p[*i].map(f64::from), p[i + 1].map(f64::from));
        (0..2).all(|k| a[k].min(b[k]) - r <= cell && a[k].max(b[k]) + r >= 0.0)
    };
    (legs.iter())
        .map(|&(lo, hi)| {
            Some((lo.max((lo..hi).find(meets)?.saturating_sub(PAD)), hi.min((lo..hi).rfind(meets)? + PAD + 1)))
        })
        .collect()
}

/// The far block of `lines`, the lines of a cell's records. An error where a run holds more than
/// `RUN_MAX` samples, or the samples more than `NAMES_MAX` names.
pub(crate) fn encode(
    cell: CellKey,
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    lines: &[WayLine],
) -> Result<Vec<u32>, String> {
    let legs: Vec<Vec<(usize, usize)>> =
        lines.iter().map(|l| legs::record_ranges(&l.spans, &l.cuts, l.p.len())).collect();
    let runs: Vec<Vec<Option<(usize, usize)>>> =
        lines.iter().zip(&legs).map(|(l, g)| runs(&l.p, g, reach(cell, l))).collect();
    let kept: Vec<usize> = (0..lines.len()).filter(|&k| runs[k].iter().any(Option::is_some)).collect();
    let mut globs: Vec<Glob> = (kept.iter())
        .flat_map(|&k| runs[k].iter().flatten().flat_map(move |&(a, b)| lines[k].key[a..=b].iter().copied()))
        .collect();
    globs.sort_unstable();
    globs.dedup();
    if globs.len() > NAMES_MAX {
        return Err(format!("cell {}/{}: {} names, more than {NAMES_MAX}", cell.x, cell.y, globs.len()));
    }
    // The samples past a run have no rank: their words are cut away.
    let order = |g: &Glob| globs.binary_search(g).map_or(u32::MAX, |k| k as u32);
    let origin = cell.origin();
    // One NaN for an unknown height: a cast's NaN bits may differ in WASM.
    let terrain = |q: [f64; 2]| {
        let g = GlobalPoint { x: origin.x + libm::round(q[0]) as i64, y: origin.y + libm::round(q[1]) as i64 };
        input.terrain.height(g).map_or(f32::NAN.to_bits(), f)
    };
    let (mut lines_w, mut samples_w, mut keys, mut sides) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for &k in &kept {
        let line = &lines[k];
        let go = line.sec.reach_m * (1.0 - rules.plan.heights.fade_share);
        for (leg, (&(lo, hi), &run)) in legs[k].iter().zip(&runs[k]).enumerate() {
            // The whole leg first: its mitres are the near block's.
            let (mut w, s) = line_words(line, (lo, hi), graph, input, rules, &order);
            let (a, n) = run.map_or((lo, 0), |(a, b)| (a, b - a + 1));
            if n > RUN_MAX {
                return Err(format!(
                    "cell {}/{}: a leg of way {} keeps {n} samples, more than {RUN_MAX}",
                    cell.x, cell.y, line.way
                ));
            }
            w[1] |= (leg as u32) << LEG_SHIFT;
            w[2] = (samples_w.len() / SAMPLE) as u32;
            w[3] = n as u32;
            lines_w.extend(w);
            keys.push(key_word(way_key(rules, line)));
            for (i, s) in (a..a + n).zip(s[SAMPLE * (a - lo)..][..SAMPLE * n].as_chunks::<SAMPLE>().0) {
                samples_w.extend(s);
                let (p, m) = (line.p[i].map(f64::from), [f32w(s[8]), f32w(s[9])]);
                let d = line.bands[i][1] + go / metres_per_unit(origin.y + i64::from(line.p[i][1]));
                sides.extend([1.0, -1.0].map(|side| terrain([p[0] + side * d * m[1], p[1] - side * d * m[0]])));
            }
        }
    }
    let mut words = vec![0u32; HEAD];
    words[0] = MAGIC_FAR;
    words[3] = keys.len() as u32;
    words[10] = (origin.y / CELL_UNITS) as u32;
    words.extend(rules.plan.heights.words());
    let at = words.len() as u32;
    words[4..7].fill(at);
    words.extend(&lines_w);
    words[7] = words.len() as u32;
    words.extend(&samples_w);
    words[8] = words.len() as u32;
    words.extend(&keys);
    words[9] = words.len() as u32;
    words.extend(&sides);
    let len = words.len() as u32;
    for k in [11, 12, 14] {
        words[k] = len;
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::MASK_M;

    #[test]
    fn the_mask_reaches_as_far_as_the_viewers() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/web/src/lib/trailview/terrain/features.wgsl");
        let features = std::fs::read_to_string(path).unwrap();
        assert!(features.contains(&format!("const MASK_M: f32 = {MASK_M};")), "MASK_M of {path}");
    }
}
