//! `obcm_diff` — compare two `.obcm` files. Parses both with the *same*
//! `obc-reader` the device uses and reports:
//!   1. structural diffs — version, bbox, marker, style table, per-LOD
//!      node/chunk counts, chunk size, max_mpp;
//!   2. feature-multiset diffs per LOD — decodes every chunk and compares the
//!      multiset of `(style_id, kind, vertices)`, since chunk/feature ordering is
//!      allowed to differ.
//!
//! Exits non-zero on any difference. `a` is the reference, `b` the candidate:
//! "only in A" means *missing from B*, "only in B" means *extra in B*. Handy for
//! checking whether a packer change altered the output.
//!
//! Usage: `obcm_diff <a.obcm> <b.obcm> [--max-examples N]`

use std::collections::HashMap;
use std::process::ExitCode;

use obc_reader::{BBox, Kind, MapCache, Reader, SliceSource, Style, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// Canonical, hashable identity of a decoded feature (geometry in microdegrees).
type FeatureKey = (u8, bool, Vec<(i32, i32)>, Vec<Vec<(i32, i32)>>);

/// Canonical form of a closed ring, invariant to **start vertex + winding**:
/// strip the closing duplicate, then take the lexicographically-smallest sequence
/// over all rotations of the ring and its reversal. Used by `--canonical-polys`
/// so that geometrically-identical closed-way polygons encoded with a different
/// ring start/direction compare equal. Lines are never canonicalized (their vertex
/// order is meaningful and matches both sides exactly).
fn canon_ring(ring: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut pts = ring.to_vec();
    if pts.len() >= 2 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n == 0 {
        return pts;
    }
    let mut best: Option<Vec<(i32, i32)>> = None;
    for reversed in [false, true] {
        let seq: Vec<(i32, i32)> = if reversed { pts.iter().rev().copied().collect() } else { pts.clone() };
        let min_pt = *seq.iter().min().unwrap();
        for i in 0..n {
            if seq[i] == min_pt {
                let cand: Vec<(i32, i32)> = seq[i..].iter().chain(seq[..i].iter()).copied().collect();
                if best.as_ref().is_none_or(|b| cand < *b) {
                    best = Some(cand);
                }
            }
        }
    }
    best.unwrap()
}

fn collect_features(r: &Reader, lod: usize, canonical: bool) -> HashMap<FeatureKey, usize> {
    // Gather every non-empty leaf (chunk_id + its node bbox) first, then decode —
    // keeps the reader borrows simple.
    let mut chunks: Vec<(u32, BBox)> = Vec::new();
    r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node)));

    let mut counts: HashMap<FeatureKey, usize> = HashMap::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for (cid, node) in chunks {
        r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
            let is_poly = f.kind == Kind::Polygon;
            let mut exterior = f.exterior().to_vec();
            let mut interiors: Vec<Vec<(i32, i32)>> = f.interiors().map(|h| h.to_vec()).collect();
            // Polygons compare up to ring rotation/winding under --canonical-polys.
            if canonical && is_poly {
                exterior = canon_ring(&exterior);
                interiors = interiors.iter().map(|h| canon_ring(h)).collect();
                interiors.sort();
            }
            let key = (f.style_id, is_poly, exterior, interiors);
            *counts.entry(key).or_insert(0) += 1;
        });
    }
    counts
}

fn style_tuple(s: &Style) -> (u8, i8, u16, u8, u8) {
    (s.id, s.z_index, s.color, s.weight, s.priority)
}

fn main() -> ExitCode {
    let mut a_path = None;
    let mut b_path = None;
    let mut max_examples = 5usize;
    let mut canonical = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--max-examples" => {
                max_examples = it.next().and_then(|s| s.parse().ok()).unwrap_or(5);
            }
            // Compare polygons up to ring rotation + winding. Lines still compare
            // exactly. Strict (byte-order) mode is the default.
            "--canonical-polys" => canonical = true,
            _ if a_path.is_none() => a_path = Some(arg),
            _ if b_path.is_none() => b_path = Some(arg),
            _ => {}
        }
    }
    let (a_path, b_path) = match (a_path, b_path) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("usage: obcm_diff <a.obcm> <b.obcm> [--max-examples N]");
            return ExitCode::FAILURE;
        }
    };

    let a_bytes = std::fs::read(&a_path).expect("read a");
    let b_bytes = std::fs::read(&b_path).expect("read b");
    let a_cache = MapCache::new();
    let b_cache = MapCache::new();
    let a_src = SliceSource(&a_bytes);
    let b_src = SliceSource(&b_bytes);
    let ra = match Reader::new(&a_src, &a_cache) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse {a_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let rb = match Reader::new(&b_src, &b_cache) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse {b_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    let mut ok = true;
    macro_rules! check {
        ($cond:expr, $($msg:tt)*) => {
            if !($cond) { ok = false; println!("DIFF  {}", format_args!($($msg)*)); }
        };
    }

    println!("== structural ==");
    if a_bytes == b_bytes {
        println!("byte-identical ({} bytes)", a_bytes.len());
    } else {
        println!("byte sizes: a={} b={}", a_bytes.len(), b_bytes.len());
    }
    check!(ra.version == rb.version, "version a={} b={}", ra.version, rb.version);
    check!(ra.bbox == rb.bbox, "bbox a={:?} b={:?}", ra.bbox, rb.bbox);
    check!(ra.marker_color == rb.marker_color, "marker a={:#06x} b={:#06x}", ra.marker_color, rb.marker_color);

    // Style table (compare all 256 slots).
    for id in 0u16..=255 {
        let sa = ra.style(id as u8).map(style_tuple);
        let sb = rb.style(id as u8).map(style_tuple);
        check!(sa == sb, "style[{id}] a={sa:?} b={sb:?}");
    }

    // LOD table.
    let la = ra.lods();
    let lb = rb.lods();
    check!(la.len() == lb.len(), "lod count a={} b={}", la.len(), lb.len());
    let n = la.len().min(lb.len());
    for i in 0..n {
        let (x, y) = (&la[i], &lb[i]);
        let mpp_eq = (x.max_mpp == y.max_mpp) || (x.max_mpp.is_infinite() && y.max_mpp.is_infinite());
        check!(mpp_eq, "lod[{i}].max_mpp a={} b={}", x.max_mpp, y.max_mpp);
        check!(x.node_count == y.node_count, "lod[{i}].node_count a={} b={}", x.node_count, y.node_count);
        check!(x.chunk_count == y.chunk_count, "lod[{i}].chunk_count a={} b={}", x.chunk_count, y.chunk_count);
        check!(x.chunk_size == y.chunk_size, "lod[{i}].chunk_size a={} b={}", x.chunk_size, y.chunk_size);
    }

    // Structural diffs are the hard failures; multiset diffs are reported with a
    // line/polygon breakdown so a caller can accept a polygon-only residual (e.g.
    // ring-winding under --canonical-polys is reconciled, leaving only GEOS
    // simplify skew) while still requiring lines to be exact.
    let structural_ok = ok;
    let mut line_diffs = 0usize;
    let mut poly_diffs = 0usize;
    let mut lod_poly_diffs: Vec<usize> = Vec::with_capacity(n);

    println!("== feature multiset (per LOD){} ==", if canonical { " [polygons canonical]" } else { "" });
    for i in 0..n {
        let ca = collect_features(&ra, i, canonical);
        let cb = collect_features(&rb, i, canonical);
        let total_a: usize = ca.values().sum();
        let total_b: usize = cb.values().sum();

        // Multiset difference both ways, split by kind (poly vs line).
        let mut only_a = 0usize;
        let mut examples_a = 0usize;
        let mut lod_poly = 0usize;
        for (k, va) in &ca {
            let vb = cb.get(k).copied().unwrap_or(0);
            if *va > vb {
                only_a += va - vb;
                if k.1 {
                    poly_diffs += va - vb;
                    lod_poly += va - vb
                } else {
                    line_diffs += va - vb
                }
                if examples_a < max_examples {
                    println!("  - LOD{i} only-in-A x{}: style={} poly={} ext_pts={}", va - vb, k.0, k.1, k.2.len());
                    examples_a += 1;
                }
            }
        }
        let mut only_b = 0usize;
        let mut examples_b = 0usize;
        for (k, vb) in &cb {
            let va = ca.get(k).copied().unwrap_or(0);
            if *vb > va {
                only_b += vb - va;
                if k.1 {
                    poly_diffs += vb - va;
                    lod_poly += vb - va
                } else {
                    line_diffs += vb - va
                }
                if examples_b < max_examples {
                    println!("  + LOD{i} only-in-B x{}: style={} poly={} ext_pts={}", vb - va, k.0, k.1, k.2.len());
                    examples_b += 1;
                }
            }
        }
        let status = if only_a == 0 && only_b == 0 { "MATCH" } else { "DIFF " };
        println!("  {status} LOD{i}: a={total_a} b={total_b} feats; only-in-A={only_a} only-in-B={only_b}");
        lod_poly_diffs.push(lod_poly);
        if only_a != 0 || only_b != 0 {
            ok = false;
        }
    }

    // Machine-readable summary for run_stage3.sh. `lod_poly_diffs` lets the gate
    // assert that no-simplify LODs match exactly (any diff there is a real bug,
    // not GEOS-version simplify skew).
    let lod_list: Vec<String> = lod_poly_diffs.iter().map(|v| v.to_string()).collect();
    println!(
        "SUMMARY structural_ok={} line_diffs={} poly_diffs={} lodpolys={}",
        structural_ok as u8,
        line_diffs,
        poly_diffs,
        lod_list.join(",")
    );

    if ok {
        println!("\nOK — no differences");
        ExitCode::SUCCESS
    } else {
        println!("\nFAILED — differences above");
        ExitCode::FAILURE
    }
}
