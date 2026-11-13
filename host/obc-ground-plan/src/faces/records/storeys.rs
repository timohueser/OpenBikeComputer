//! The storeys over the ground in a cell's one near block: each storey's records follow storey 0's in every
//! table, so a corner's label names a label of any storey. A storey's lines (`STOREY`, after the ceiling lines)
//! are its own; its height segments, discs, blend edges and centre-line segments come last in each bin's lists.
//! The bins list no candidate item of a storey over the ground: the mesh names its faces' labels, and no reader
//! decides one by point.

use super::{encode, BEYOND, BLEND, CAND, DISC, HEADER, JR, JR_ARM, LABEL, LEG_SHIFT, LINE, SAMPLE, STOREY};
use super::{BINS, JUNCTION};
use crate::faces::deck::{DeckWay, Decks, Storeys, PIER_NEAR_M};
use crate::faces::{candidates, window, Rules, Sliced, LAYERS};
use crate::frame::CellKey;
use crate::graph::WayGraph;
use crate::tile::Geometry;
use crate::CellInput;

/// The storeys over the ground near the cell, storey 1 first, with their records merged into `near` (storey 0's
/// block). A storey with no deck near the cell has no plan; without the feature `storeys` there is none.
pub fn storeys(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, near: &mut Vec<u32>) -> Storeys {
    if !cfg!(feature = "storeys") {
        return Storeys::default();
    }
    let top = Decks::of(graph).storey.iter().copied().max().unwrap_or(0);
    let mut out = Storeys::default();
    for k in 1..=top {
        let c = candidates(graph, input, rules, cell, k, LAYERS, true);
        if c.cands.is_empty() {
            continue;
        }
        let s = Sliced::new(&c);
        let faces = s.cell(&c, cell, k);
        let plan = s.plan(cell, k, &s.map(), &s.labels);
        let sub = encode(cell, graph, input, rules, (&c.lines, &[]), &c.cands, &plan, false);
        *near = merge(near, &sub.words, k);
        for w in faces.faces.iter().filter_map(|f| f.material) {
            if let Err(k) = out.ways.binary_search_by_key(&w, |x| x.0) {
                out.ways.insert(k, (w, DeckWay::of(graph, input, rules, w)));
            }
        }
        out.plans.push(faces);
    }
    if !out.plans.is_empty() {
        let o = cell.origin();
        let near_m = window(cell, PIER_NEAR_M);
        for f in input.features.iter().filter(|f| f.kind == "bridge-pier" && f.geometry == Geometry::Point) {
            for p in f.parts.iter().filter_map(|p| p.points.first()) {
                let at = [(p.x - o.x) as i32, (p.y - o.y) as i32];
                if (near_m[0]..=near_m[2]).contains(&at[0])
                    && (near_m[1]..=near_m[3]).contains(&at[1])
                    && !out.piers.contains(&at)
                {
                    out.piers.push(at);
                }
            }
        }
    }
    out
}

fn add(v: u32, by: u32) -> u32 {
    if v == u32::MAX {
        v
    } else {
        v + by
    }
}

/// Block `main` with the records of block `sub` of storey `storey` after its own in every table, their indices
/// moved past main's. Main's words keep their values, its offsets move.
fn merge(main: &[u32], sub: &[u32], storey: i8) -> Vec<u32> {
    let (c0, l0, n0) = (main[1], main[2], main[3]);
    let x0 = main[12] - main[8];
    let j0 = main[13];
    let e0 = (main[14] - main[11]) / BLEND as u32;
    let s0 = (main[8] - main[7]) / SAMPLE as u32;
    let mut w = main[..main[4] as usize].to_vec();
    w[1] += sub[1];
    w[2] += sub[2];
    w[3] += sub[3];
    // Candidates: the label, the line and the offsets into the extras.
    w.extend(&main[main[4] as usize..main[5] as usize]);
    let sub_cands = &sub[sub[4] as usize..sub[5] as usize];
    for k in sub_cands.chunks(CAND) {
        let mut k = k.to_vec();
        k[3] = add(k[3], l0);
        k[4] = add(k[4], n0);
        for at in [5, 7, 9] {
            k[at] += x0;
        }
        w.extend(k);
    }
    w[5] = w.len() as u32;
    w.extend(&main[main[5] as usize..main[6] as usize]);
    for l in sub[sub[5] as usize..sub[6] as usize].chunks(LABEL) {
        let mut l = l.to_vec();
        if (l[0] >> 8) & 0xff == JUNCTION {
            l[1] += j0;
        }
        for x in &mut l[2..] {
            *x = add(*x, n0);
        }
        w.extend(l);
    }
    w[6] = w.len() as u32;
    w.extend(&main[main[6] as usize..main[7] as usize]);
    for l in sub[sub[6] as usize..sub[7] as usize].chunks(LINE) {
        let mut l = l.to_vec();
        l[1] |= STOREY | u32::from(storey as u8) << LEG_SHIFT;
        l[2] += s0;
        w.extend(l);
    }
    w[7] = w.len() as u32;
    w.extend(&main[main[7] as usize..main[8] as usize]);
    w.extend(&sub[sub[7] as usize..sub[8] as usize]);
    w[8] = w.len() as u32;
    w.extend(&main[main[8] as usize..main[12] as usize]);
    let mut extras = sub[sub[8] as usize..sub[12] as usize].to_vec();
    for k in sub_cands.chunks(CAND) {
        for d in 0..k[6] as usize {
            let at = k[5] as usize + DISC * d;
            extras[at + 3] = add(extras[at + 3], n0);
            extras[at + 4] = add(extras[at + 4], n0);
        }
        for b in 0..k[8] as usize {
            let at = k[7] as usize + BEYOND * b + 4;
            extras[at] = add(extras[at], l0);
        }
    }
    w.extend(extras);
    // Junctions: a table of offsets, then the records, whose arms name their lines in the low half word.
    w[12] = w.len() as u32;
    w[13] = j0 + sub[13];
    let record = |b: &[u32], k: u32| {
        let at = b[b[12] as usize + k as usize] as usize;
        b[at..at + JR + JR_ARM * (b[at + 2] & 0xff) as usize].to_vec()
    };
    let mut records: Vec<Vec<u32>> = (0..j0).map(|k| record(main, k)).collect();
    for k in 0..sub[13] {
        let mut r = record(sub, k);
        for a in r[JR..].chunks_mut(JR_ARM) {
            a[0] = (a[0] & !0xffff) | ((a[0] & 0xffff) + n0);
        }
        records.push(r);
    }
    let mut at = w.len() + records.len();
    for r in &records {
        w.push(at as u32);
        at += r.len();
    }
    w.extend(records.iter().flatten());
    w[11] = w.len() as u32;
    w.extend(&main[main[11] as usize..main[14] as usize]);
    for b in sub[sub[11] as usize..sub[14] as usize].chunks(BLEND) {
        w.extend(&b[..4]);
        w.extend([add(b[4], l0), add(b[5], l0)]);
    }
    w[14] = w.len() as u32;
    w.extend(&main[main[14] as usize..main[9] as usize]);
    // The bins: main's items; each list with the storey's after main's; main's outline edges.
    w[9] = w.len() as u32;
    let heads = w.len();
    let n_bins = (BINS * BINS) as usize;
    w.resize(heads + HEADER * n_bins, 0);
    let outline = main[14] < main[9];
    for b in 0..n_bins {
        let lists = |x: &[u32], outline: bool| -> [Vec<u32>; 6] {
            let h = x[9] as usize + HEADER * b;
            if x[h] == 0 {
                return Default::default();
            }
            let at = x[h] as usize;
            let (n_items, n_segs) = ((x[h + 3] >> 16) as usize, (x[h + 1] & 0xffff) as usize);
            let (n_hs, n_hd, n_be) =
                ((x[h + 2] >> 16) as usize, (x[h + 2] & 0xffff) as usize, (x[h + 3] & 0xffff) as usize);
            let mut k = at;
            let mut take = |n: usize| {
                let v = x[k..k + n].to_vec();
                k += n;
                v
            };
            let (items, segs, hs, hd, be) = (take(n_items), take(n_segs), take(n_hs), take(n_hd), take(n_be));
            let rest = if outline {
                let n = (x[k] & 0xffff) as usize;
                x[k..k + (n + 2) / 2].to_vec()
            } else {
                Vec::new()
            };
            [items, segs, hs, hd, be, rest]
        };
        let [items, mut segs, mut hs, mut hd, mut be, rest] = lists(main, outline);
        let [_, s_segs, s_hs, s_hd, s_be, _] = lists(sub, false);
        let line = |v: &u32| (v & !0xffff) | ((v & 0xffff) + n0);
        segs.extend(s_segs.iter().map(line));
        hs.extend(s_hs.iter().map(line));
        hd.extend(s_hd.iter().map(|v| (v & !0xffff) | ((v & 0xffff) + c0)));
        be.extend(s_be.iter().map(|v| v + e0));
        if items.is_empty() && segs.is_empty() && hs.is_empty() && hd.is_empty() && be.is_empty() && rest.is_empty() {
            continue;
        }
        assert!(segs.len() < 1 << 16 && hs.len() < 1 << 16 && hd.len() < 1 << 16 && be.len() < 1 << 16);
        let n_items = (main[main[9] as usize + HEADER * b + 1] >> 16) * u32::from(!items.is_empty());
        let h = heads + HEADER * b;
        w[h] = w.len() as u32;
        w[h + 1] = n_items << 16 | segs.len() as u32;
        w[h + 2] = (hs.len() as u32) << 16 | hd.len() as u32;
        w[h + 3] = (items.len() as u32) << 16 | be.len() as u32;
        w.extend(items.iter().chain(&segs).chain(&hs).chain(&hd).chain(&be));
        // A block with outline edges lists their count in every bin it lists.
        if outline {
            w.extend(if rest.is_empty() { vec![0] } else { rest });
        }
    }
    w
}
