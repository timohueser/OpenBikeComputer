//! `obcm_diff` — compare two `.obcm` files. Parses both with the same `obc-reader`
//! the device uses and reports:
//!   1. structural diffs — version, bbox, marker, style table, per-LOD
//!      node/chunk counts, chunk size, max_mpp;
//!   2. feature-multiset diffs per LOD — decodes every chunk and compares the
//!      multiset of `(style_id, kind, vertices)`, since chunk/feature ordering is
//!      allowed to differ.
//!
//! Exits non-zero on any difference. `a` is the reference, `b` the candidate:
//! "only in A" means missing from B, "only in B" means extra in B.
//!
//! `--dump` takes **one** file and writes a canonical, sorted text listing of its
//! *content* to stdout — the diffable form of the same comparison, for the case two
//! maps cannot both be parsed by one binary: a **format migration**. Build the tool
//! before and after the bump, dump the same extract with each, and `diff` the two
//! listings; equal listings say the migration moved bytes and not content.
//! Deliberately excluded from the listing, because a version bump is expected to
//! change them: the version byte, the file length, and every *addressing* field
//! (chunk ids, chunk byte offsets, feature offsets within a chunk, section offsets).
//! Tree shape is content here — node/chunk counts are listed, since a change in how
//! chunks are laid out must not move a leaf.
//!
//! Usage: `obcm_diff <a.obcm> <b.obcm> [--max-examples N]`
//!        `obcm_diff <map.obcm> --dump`

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use obc_reader::{
    BBox, Kind, MapCache, MapTables, PoiCategory, Reader, SliceSource, Style, MAX_FEAT_PTS, MAX_FEAT_RINGS,
    MAX_POI_RESULTS, NAV_MAX_CHUNK_BYTES,
};

/// Canonical, hashable identity of a decoded feature (geometry in microdegrees).
type FeatureKey = (u8, bool, Vec<(i32, i32)>, Vec<Vec<(i32, i32)>>);

/// Canonical form of a closed ring, invariant to start vertex + winding: strip the
/// closing duplicate, then take the lexicographically-smallest sequence over all
/// rotations of the ring and its reversal. Used by `--canonical-polys` so equal
/// polygons with different ring start/direction compare equal. Lines are never
/// canonicalized (vertex order is meaningful).
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
    // Gather every non-empty leaf first, then decode — keeps the reader borrows simple.
    let mut chunks: Vec<(u32, BBox)> = Vec::new();
    r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node))).unwrap();

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
        })
        .unwrap();
    }
    counts
}

fn style_tuple(s: &Style) -> (u8, i8, u16, u8, u8) {
    (s.id, s.z_index, s.color, s.weight, s.priority)
}

/// Render one feature key as a single canonical line. Vertices are absolute microdegrees, so the
/// line is independent of how the anchor was encoded — the whole point when the header layout is
/// what changed.
fn feature_line(key: &FeatureKey, count: usize) -> String {
    let (style_id, is_poly, exterior, interiors) = key;
    let pts =
        |ring: &[(i32, i32)]| -> String { ring.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ") };
    let mut line = format!(
        "  feat style={style_id} kind={} n={count} ext[{}]={}",
        if *is_poly { "poly" } else { "line" },
        exterior.len(),
        pts(exterior)
    );
    for hole in interiors {
        line.push_str(&format!(" hole[{}]={}", hole.len(), pts(hole)));
    }
    line
}

/// Write a canonical content listing of one map to stdout (see the module docs for what is
/// deliberately left out). Every list is sorted, so the output is stable across runs and across
/// packer versions that reorder chunks.
fn dump(r: &Reader, path: &str) {
    println!("== dump {path} ==");
    println!("bbox {} {} {} {}", r.bbox.min_lon, r.bbox.min_lat, r.bbox.max_lon, r.bbox.max_lat);
    println!("marker {:#06x}", r.marker_color);
    for id in 0u16..=255 {
        if let Some(s) = r.style(id as u8) {
            println!(
                "style id={} z={} color={:#06x} weight={} prio={} dashed={} color2={}",
                s.id,
                s.z_index,
                s.color,
                s.weight,
                s.priority,
                s.dashed as u8,
                s.color2.map_or("-".into(), |c| format!("{c:#06x}"))
            );
        }
    }

    // Geometry: the tree shape plus the sorted feature multiset, per LOD.
    for (i, l) in r.lods().iter().enumerate() {
        println!(
            "lod {i} max_mpp={} nodes={} chunks={} chunk_size={}",
            l.max_mpp, l.node_count, l.chunk_count, l.chunk_size
        );
        let counts = collect_features(r, i, false);
        let mut lines: Vec<String> = counts.iter().map(|(k, &n)| feature_line(k, n)).collect();
        lines.sort();
        let total: usize = counts.values().sum();
        for line in &lines {
            println!("{line}");
        }
        println!("lod {i} features={total} distinct={}", lines.len());
    }

    // POI section. There is no whole-section enumeration in the reader's API (the device only ever
    // asks for nearest-N), so this lists the directory shape plus the nearest-16 of every category
    // to the map's centre — a deterministic content probe over the records, not a full dump.
    let dir = r.poi_directory();
    println!("poi chunk_size={} hours_pool_count={}", dir.chunk_size, dir.hours_pool_count);
    for e in dir.entries.iter() {
        println!("poi cat={} nodes={} chunks={}", e.category_id, e.node_count, e.chunk_count);
    }
    let centre = ((r.bbox.min_lon + r.bbox.max_lon) / 2, (r.bbox.min_lat + r.bbox.max_lat) / 2);
    let mut out = heapless::Vec::<_, MAX_POI_RESULTS>::new();
    for cat in PoiCategory::ALL {
        if r.nearest_pois(cat, centre, &mut out).is_err() {
            println!("poi nearest cat={} ERROR", cat.id());
            continue;
        }
        for p in out.iter() {
            println!(
                "poi nearest cat={} subtype={} lat={} lon={} hours_ref={} dist={} name={:?}",
                cat.id(),
                p.subtype,
                p.lat,
                p.lon,
                p.hours_ref,
                p.distance_m,
                p.name.as_str()
            );
        }
    }

    // Nav graph: the directory shape, the profile table, and every junction record. Bin-packed
    // chunks can hand the same record to several leaves (spec §8.3), so dedup by node id.
    let nav = r.nav_directory();
    println!("nav nodes={} chunks={} edge_chunks={}", nav.node_count, nav.chunk_count, nav.edge_chunk_count);
    for p in r.nav_profiles() {
        println!("nav profile name={:?} highway={:?} surface={:?}", p.name(), p.highway, p.surface);
    }
    let mut scratch = [0u8; NAV_MAX_CHUNK_BYTES];
    let mut node_lines: Vec<String> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut edge_ids: Vec<u32> = Vec::new();
    let view = r.bbox;
    if let Err(e) = r.for_each_nav_node(&view, &mut scratch, |n| {
        if !seen.insert(n.id) {
            return;
        }
        let mut line = format!("nav node id={} lat={} lon={} deg={}", n.id, n.lat, n.lon, n.degree());
        for nb in n.neighbors() {
            line.push_str(&format!(" [{} {} {} {} {}]", nb.id, nb.lat, nb.lon, nb.cost_m, nb.way_kind));
            edge_ids.push(nb.edge_id);
        }
        node_lines.push(line);
    }) {
        println!("nav walk ERROR {e:?}");
    }
    node_lines.sort();
    for line in &node_lines {
        println!("{line}");
    }
    println!("nav junctions={}", node_lines.len());

    // Edge geometry, keyed by the pool-relative id the adjacency entries carry (the id is
    // addressing, so it is not printed — only the polyline it resolves to).
    edge_ids.sort_unstable();
    edge_ids.dedup();
    let mut edge_lines: Vec<String> = Vec::with_capacity(edge_ids.len());
    let mut poly = heapless::Vec::<_, 256>::new();
    for id in edge_ids {
        match r.nav_edge(id, &mut poly) {
            Some(length_m) => edge_lines.push(format!(
                "nav edge len={length_m} pts={} {}",
                poly.len(),
                poly.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ")
            )),
            None => edge_lines.push("nav edge UNRESOLVED".into()),
        }
    }
    edge_lines.sort();
    for line in &edge_lines {
        println!("{line}");
    }
    println!("nav edges={}", edge_lines.len());
}

/// One direction of the per-LOD multiset difference: every key whose count in
/// `src` exceeds its count in `other` (the excess present only in `src`). Prints up
/// to `max_examples` of them with `symbol`/`label` (e.g. `-`/`only-in-A`) and
/// returns `(total_excess, poly_excess, line_excess)` — split by the key's
/// poly-vs-line bit (`k.1`).
fn directional_diff(
    src: &HashMap<FeatureKey, usize>,
    other: &HashMap<FeatureKey, usize>,
    lod: usize,
    symbol: char,
    label: &str,
    max_examples: usize,
) -> (usize, usize, usize) {
    let (mut only, mut poly, mut line, mut examples) = (0usize, 0usize, 0usize, 0usize);
    for (k, &vs) in src {
        let vo = other.get(k).copied().unwrap_or(0);
        if vs > vo {
            let excess = vs - vo;
            only += excess;
            if k.1 {
                poly += excess;
            } else {
                line += excess;
            }
            if examples < max_examples {
                println!("  {symbol} LOD{lod} {label} x{excess}: style={} poly={} ext_pts={}", k.0, k.1, k.2.len());
                examples += 1;
            }
        }
    }
    (only, poly, line)
}

fn main() -> ExitCode {
    let mut a_path = None;
    let mut b_path = None;
    let mut max_examples = 5usize;
    let mut canonical = false;
    let mut dump_only = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--max-examples" => {
                max_examples = it.next().and_then(|s| s.parse().ok()).unwrap_or(5);
            }
            // Compare polygons up to ring rotation + winding. Lines still compare
            // exactly. Strict (byte-order) mode is the default.
            "--canonical-polys" => canonical = true,
            // One file in, a canonical content listing out (see the module docs).
            "--dump" => dump_only = true,
            _ if a_path.is_none() => a_path = Some(arg),
            _ if b_path.is_none() => b_path = Some(arg),
            _ => {}
        }
    }

    if dump_only {
        let path = match a_path {
            Some(p) => p,
            None => {
                eprintln!("usage: obcm_diff <map.obcm> --dump");
                return ExitCode::FAILURE;
            }
        };
        let bytes = std::fs::read(&path).expect("read map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let tables = match MapTables::parse(&src) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("parse {path}: {e:?}");
                return ExitCode::FAILURE;
            }
        };
        dump(&Reader::new(&src, &tables, &cache), &path);
        return ExitCode::SUCCESS;
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
    let a_tables = match MapTables::parse(&a_src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("parse {a_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let b_tables = match MapTables::parse(&b_src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("parse {b_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let ra = Reader::new(&a_src, &a_tables, &a_cache);
    let rb = Reader::new(&b_src, &b_tables, &b_cache);

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
        let (only_a, poly_a, line_a) = directional_diff(&ca, &cb, i, '-', "only-in-A", max_examples);
        let (only_b, poly_b, line_b) = directional_diff(&cb, &ca, i, '+', "only-in-B", max_examples);
        poly_diffs += poly_a + poly_b;
        line_diffs += line_a + line_b;
        let lod_poly = poly_a + poly_b;
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
