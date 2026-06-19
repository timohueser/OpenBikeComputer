//! `obcm_diff` — the escalating-comparison tool from the plan's §5, the gate for
//! every stage past serialize. Parses two `.obcm` files with the *same*
//! `obc-reader` the device uses and reports:
//!   1. structural diffs — version, bbox, marker, style table, per-LOD
//!      node/chunk counts, chunk size, max_mpp;
//!   2. feature-multiset diffs per LOD — decodes every chunk and compares the
//!      multiset of `(style_id, kind, vertices)`, since ordering is allowed to
//!      differ (see the corpus README's validation-strategy note).
//!
//! Exits non-zero on any difference. `a` is treated as the reference (oracle),
//! `b` as the candidate (Rust); "only in A" therefore means *missing from the
//! Rust output*, "only in B" means *extra*.
//!
//! Usage: `obcm_diff <a.obcm> <b.obcm> [--max-examples N]`

use std::collections::HashMap;
use std::process::ExitCode;

use obc_reader::{BBox, Kind, Reader, Style, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// Canonical, hashable identity of a decoded feature (geometry in microdegrees).
type FeatureKey = (u8, bool, Vec<(i32, i32)>, Vec<Vec<(i32, i32)>>);

fn collect_features(r: &Reader, lod: usize) -> HashMap<FeatureKey, usize> {
    // Gather every non-empty leaf (chunk_id + its node bbox) first, then decode —
    // keeps the reader borrows simple.
    let mut chunks: Vec<(u32, BBox)> = Vec::new();
    r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node)));

    let mut counts: HashMap<FeatureKey, usize> = HashMap::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for (cid, node) in chunks {
        r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
            let exterior = f.exterior().to_vec();
            let interiors: Vec<Vec<(i32, i32)>> = f.interiors().map(|h| h.to_vec()).collect();
            let key = (f.style_id, f.kind == Kind::Polygon, exterior, interiors);
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
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--max-examples" => {
                max_examples = it.next().and_then(|s| s.parse().ok()).unwrap_or(5);
            }
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
    let ra = match Reader::new(&a_bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse {a_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let rb = match Reader::new(&b_bytes) {
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

    println!("== feature multiset (per LOD) ==");
    for i in 0..n {
        let ca = collect_features(&ra, i);
        let cb = collect_features(&rb, i);
        let total_a: usize = ca.values().sum();
        let total_b: usize = cb.values().sum();

        // Multiset difference both ways.
        let mut only_a = 0usize;
        let mut examples_a = 0usize;
        for (k, va) in &ca {
            let vb = cb.get(k).copied().unwrap_or(0);
            if *va > vb {
                only_a += va - vb;
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
                if examples_b < max_examples {
                    println!("  + LOD{i} only-in-B x{}: style={} poly={} ext_pts={}", vb - va, k.0, k.1, k.2.len());
                    examples_b += 1;
                }
            }
        }
        let status = if only_a == 0 && only_b == 0 { "MATCH" } else { "DIFF " };
        println!("  {status} LOD{i}: a={total_a} b={total_b} feats; only-in-A={only_a} only-in-B={only_b}");
        if only_a != 0 || only_b != 0 {
            ok = false;
        }
    }

    if ok {
        println!("\nOK — no differences");
        ExitCode::SUCCESS
    } else {
        println!("\nFAILED — differences above");
        ExitCode::FAILURE
    }
}
