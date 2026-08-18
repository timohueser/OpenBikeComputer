//! `cell_size_survey` — measure an `.obcm` the way a **cell catalog** would have to store it
//! (epic #1016, deliverable D1).
//!
//! The cell design's one free parameter is the cell *size* per band, and the only honest way to
//! choose it is to look at where the bytes actually are in a real bake. This example reads a packed
//! map through the same `obc-reader` the device uses and reports two things:
//!
//! 1. **Section budget** — per-LOD region bytes (index + offset table + chunk data, the three parts
//!    of OBCM_Spec §3), plus the POI section, the hours pool, and the nav-graph section. This says
//!    which LODs are worth cell-splitting at all and how big a *core file* (styles + nav + POIs, and
//!    no geometry at all — OBCA_Spec §5.1) would be, which is the one size in a volume set that
//!    cannot be reduced by splitting.
//! 2. **Per-cell byte distribution** — for each candidate cell size, the LOD's chunk bytes binned
//!    into cells of the fixed global grid (`--grid-origin`, default −2^28 µdeg on both axes), so a
//!    shard-count and fetch-count model has a measured distribution rather than an average.
//!
//! Binning is **area-proportional**: a leaf's bytes are split across the cells it overlaps in
//! proportion to the overlapped area. For a fine LOD, whose leaves are far smaller than a cell,
//! that is exact — every leaf lands wholly inside one cell. For a coarse LOD it is a model, and a
//! deliberately conservative one: real per-cell bakes re-simplify and re-split, so a cut leaf's two
//! halves cost slightly *more* than the fractions reported here (two chunk headers, two sentinels).
//! The nav section is binned by junction: each §8.3 record's exact byte length lands in its own
//! coordinate's cell, and the edge pool is pro-rated by each cell's share of junctions (an edge
//! record is shared by two endpoints, and fetching millions of them to attribute exactly would cost
//! more than the precision is worth).
//!
//! Read-only and additive: no packer or reader behaviour is touched. The band sizes it produced —
//! `2^20` coarse / `2^19` mid / `2^18` fine + network — are normative in
//! [`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §1.5 as **schema data**, so re-running this on new
//! bakes is how those numbers get retuned.
//!
//! ```sh
//! cargo run --release --example cell_size_survey -- switzerland.obcm
//! cargo run --release --example cell_size_survey -- a.obcm b.obcm --cells 19,20,21 --no-nav
//! ```

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, Reader, SliceSource};

/// Origin of the fixed global cell grid, in microdegrees, on **both** axes (epic #1016 §1). A
/// power of two so that every candidate cell size divides the origin offset exactly, and negative
/// enough to contain the whole geographic domain (±90e6 / ±180e6) — the grid is defined over a
/// square µdeg world because an OBCM quadtree halves both axes together, so cells must be square
/// in µdeg for a cell to ever coincide with a quadtree node.
const DEFAULT_GRID_ORIGIN: i64 = -(1 << 28);

/// Candidate cell sizes, as `log2(µdeg)`: 2^18 ≈ 0.26°, 2^19 ≈ 0.52°, 2^20 ≈ 1.05°, 2^21 ≈ 2.10°.
/// The default spans the three sizes the v1 band table settled on plus the next one up, so a plain
/// run reproduces the table and shows what the rejected step would have cost.
const DEFAULT_CELL_LOG2: &[u32] = &[18, 19, 20, 21];

/// Bytes attributed to one cell, keyed by `(lat index, lon index)` on the grid.
type CellBytes = HashMap<(i64, i64), f64>;

struct Options {
    paths: Vec<String>,
    cell_log2: Vec<u32>,
    grid_origin: i64,
    nav: bool,
}

fn usage() -> &'static str {
    "usage: cell_size_survey <map.obcm>... [--cells 19,20,21] [--grid-origin <udeg>] [--no-nav]"
}

fn parse_args() -> Result<Options, String> {
    let mut opts = Options {
        paths: Vec::new(),
        cell_log2: DEFAULT_CELL_LOG2.to_vec(),
        grid_origin: DEFAULT_GRID_ORIGIN,
        nav: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--cells" => {
                let v = args.next().ok_or_else(|| "--cells needs a value".to_string())?;
                opts.cell_log2 = v
                    .split(',')
                    .map(|s| s.trim().parse::<u32>().map_err(|e| format!("--cells {s:?}: {e}")))
                    .collect::<Result<_, _>>()?;
                if opts.cell_log2.iter().any(|&b| !(4..=30).contains(&b)) {
                    return Err("--cells entries are log2(µdeg) and must be in 4..=30".into());
                }
            }
            "--grid-origin" => {
                let v = args.next().ok_or_else(|| "--grid-origin needs a value".to_string())?;
                opts.grid_origin = v.parse::<i64>().map_err(|e| format!("--grid-origin {v:?}: {e}"))?;
            }
            "--no-nav" => opts.nav = false,
            "-h" | "--help" => return Err(usage().into()),
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}\n{}", usage())),
            other => opts.paths.push(other.to_string()),
        }
    }
    if opts.paths.is_empty() {
        return Err(usage().into());
    }
    Ok(opts)
}

/// Split `bytes` across the grid cells a leaf's bbox overlaps, in proportion to overlapped area.
/// The bbox's `max` edges are treated as **exclusive** (a quadtree splits at a shared midpoint, so
/// counting the far edge would credit a whole extra row of cells to every leaf).
fn distribute(node: &BBox, bytes: f64, cell: i64, origin: i64, out: &mut CellBytes) {
    let (lat0, lat1) = (node.min_lat as i64, node.max_lat as i64);
    let (lon0, lon1) = (node.min_lon as i64, node.max_lon as i64);
    let (span_lat, span_lon) = ((lat1 - lat0).max(1), (lon1 - lon0).max(1));
    let area = span_lat as f64 * span_lon as f64;
    let i_lo = (lat0 - origin).div_euclid(cell);
    let i_hi = (lat0 + span_lat - 1 - origin).div_euclid(cell);
    let j_lo = (lon0 - origin).div_euclid(cell);
    let j_hi = (lon0 + span_lon - 1 - origin).div_euclid(cell);
    for i in i_lo..=i_hi {
        let (c_lat0, c_lat1) = (origin + i * cell, origin + (i + 1) * cell);
        let ov_lat = (lat1.min(c_lat1) - lat0.max(c_lat0)).max(0);
        if ov_lat == 0 {
            continue;
        }
        for j in j_lo..=j_hi {
            let (c_lon0, c_lon1) = (origin + j * cell, origin + (j + 1) * cell);
            let ov_lon = (lon1.min(c_lon1) - lon0.max(c_lon0)).max(0);
            if ov_lon == 0 {
                continue;
            }
            *out.entry((i, j)).or_insert(0.0) += bytes * (ov_lat as f64 * ov_lon as f64) / area;
        }
    }
}

/// One percentile of an already-sorted slice, by nearest rank.
fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[k.min(sorted.len() - 1)]
}

fn mib(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

/// `12.3 MiB` / `456 KiB` / `789 B`, whichever reads best — these numbers span six orders of
/// magnitude across a survey and a fixed unit makes half the table unreadable.
fn human(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", mib(bytes))
    } else if bytes >= 1024.0 {
        format!("{:.0} KiB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    }
}

/// Print `name`'s per-cell distribution for one cell size.
fn report_cells(name: &str, cells: &CellBytes) {
    let mut v: Vec<f64> = cells.values().copied().filter(|b| *b > 0.0).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = v.iter().sum();
    let mean = if v.is_empty() { 0.0 } else { total / v.len() as f64 };
    println!(
        "  {name:<10} cells {:>5}  total {:>10}  mean {:>10}  p50 {:>10}  p90 {:>10}  p99 {:>10}  max {:>10}",
        v.len(),
        human(total),
        human(mean),
        human(pct(&v, 50.0)),
        human(pct(&v, 90.0)),
        human(pct(&v, 99.0)),
        human(pct(&v, 100.0)),
    );
}

/// Read the `k`-th entry of a LOD's v11 offset table (§5.1) straight out of the file bytes. The
/// reader keeps its chunk-extent math private (it is a bounds-checked internal), and a survey wants
/// every chunk's length rather than one chunk's extent, so the table is read here directly.
fn offset_entry(bytes: &[u8], table_start: usize, k: usize) -> Option<u32> {
    let at = table_start.checked_add(k.checked_mul(4)?)?;
    let raw: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

fn survey(path: &str, opts: &Options) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).map_err(|e| format!("{path}: parse: {e:?}"))?;
    let cache = MapCache::new_boxed();
    let r = Reader::new(&src, &tables, &cache);

    let bbox = r.bbox;
    let span_lat_deg = (bbox.max_lat - bbox.min_lat) as f64 / 1e6;
    let span_lon_deg = (bbox.max_lon - bbox.min_lon) as f64 / 1e6;
    let bbox_deg2 = span_lat_deg * span_lon_deg;

    println!("== survey {path} ==");
    println!("file {} B ({})", bytes.len(), human(bytes.len() as f64));
    println!(
        "bbox lat {:.5}..{:.5} lon {:.5}..{:.5}  ({span_lat_deg:.3}° × {span_lon_deg:.3}° = {bbox_deg2:.3} deg²)",
        bbox.min_lat as f64 / 1e6,
        bbox.max_lat as f64 / 1e6,
        bbox.min_lon as f64 / 1e6,
        bbox.max_lon as f64 / 1e6,
    );

    // --- 1. Section budget -------------------------------------------------------------------
    //
    // Per LOD, the three parts of the region (§3) plus a byte density, so LODs are comparable
    // across maps of different size.
    println!("\n-- per-LOD regions --");
    println!(
        "{:>3}  {:>9}  {:>7}  {:>9}  {:>10}  {:>11}  {:>10}  {:>9}",
        "LOD", "max_mpp", "chunks", "index", "offs table", "chunk data", "total", "MiB/deg²"
    );
    let mut lod_totals: Vec<f64> = Vec::new();
    let mut geom_total = 0.0f64;
    for (i, l) in r.lods().iter().enumerate() {
        let index_b = (l.node_count * 4) as f64;
        let table_b = ((l.chunk_count + 1) * 4) as f64;
        let chunk_b = l.scale.offset(l.chunk_units_total).bytes() as f64;
        let total = index_b + table_b + chunk_b;
        lod_totals.push(total);
        geom_total += total;
        let mpp = if l.max_mpp.is_finite() { format!("{:.1}", l.max_mpp) } else { "inf".to_string() };
        println!(
            "{i:>3}  {mpp:>9}  {:>7}  {:>9}  {:>10}  {:>11}  {:>10}  {:>9.2}",
            l.chunk_count,
            human(index_b),
            human(table_b),
            human(chunk_b),
            human(total),
            if bbox_deg2 > 0.0 { mib(total) / bbox_deg2 } else { 0.0 },
        );
    }

    // POI + hours + nav are the sections a *core file* must carry whole; size them separately.
    let poi = r.poi_directory();
    let poi_start = poi.entries.iter().map(|e| e.index_offset).min().unwrap_or(0);
    let poi_end = poi.hours_pool_offset;
    let hours_b = 2.0 + 29.0 * poi.hours_pool_count as f64;
    let nav = r.nav_directory();
    let nav_start = nav.profile_table_offset;
    let nav_b = (bytes.len() as u64 - nav_start.min(bytes.len() as u64)) as f64;
    let poi_b = poi_end.saturating_sub(poi_start) as f64;
    println!("\n-- non-geometry sections --");
    println!("  header+styles+lod table  {:>10}", human(r.lods().first().map_or(0, |l| l.index_offset) as f64));
    println!("  POI section              {:>10}  ({} categories)", human(poi_b), poi.entries.len());
    println!("  hours pool               {:>10}  ({} blobs)", human(hours_b), poi.hours_pool_count);
    println!(
        "  nav graph (§8)           {:>10}  ({} node chunks, {} edge chunks)",
        human(nav_b),
        nav.chunk_count,
        nav.edge_chunk_count
    );
    println!(
        "  geometry (all LODs)      {:>10}  ({:.1}% of file)",
        human(geom_total),
        100.0 * geom_total / bytes.len() as f64
    );

    // --- 2. Per-cell distributions ------------------------------------------------------------
    //
    // Walk each LOD's leaves once per LOD and bin into every candidate cell size in the same pass.
    let mut leaves: Vec<Vec<(BBox, f64)>> = Vec::new();
    for (i, l) in r.lods().iter().enumerate() {
        let table_start = (l.index_offset + (l.node_count * 4) as u64) as usize;
        let mut per_lod: Vec<(BBox, f64)> = Vec::new();
        let mut bad = 0usize;
        r.for_each_chunk(i, &bbox, |cid, node| {
            match (offset_entry(&bytes, table_start, cid as usize), offset_entry(&bytes, table_start, cid as usize + 1))
            {
                (Some(a), Some(b)) if b >= a => per_lod.push((node, (b - a) as f64)),
                _ => bad += 1,
            }
        })
        .map_err(|e| format!("{path}: LOD {i} walk: {e:?}"))?;
        if bad > 0 {
            return Err(format!("{path}: LOD {i}: {bad} leaves with an unreadable offset pair"));
        }
        leaves.push(per_lod);
    }

    // Nav: junction record bytes exactly per cell, edge pool pro-rated by junction share.
    let mut nav_nodes: Vec<(i32, i32, f64)> = Vec::new();
    if opts.nav && !nav.is_empty() {
        let mut scratch = vec![0u8; nav.chunk_size];
        let mut seen: HashSet<u32> = HashSet::new();
        // §8.2 bin-packing means a leaf walk can yield the same record more than once — dedupe by
        // `Node Id`, exactly as A* settle does.
        r.for_each_nav_node(&bbox, &mut scratch, |n| {
            if seen.insert(n.id) {
                nav_nodes.push((n.lat, n.lon, (13 + 15 * n.degree()) as f64));
            }
        })
        .map_err(|e| format!("{path}: nav walk: {e:?}"))?;
        let node_b: f64 = nav_nodes.iter().map(|(_, _, b)| b).sum();
        let edge_b = (nav.edge_chunk_count * nav.chunk_size) as f64;
        println!(
            "  nav junctions            {:>10}  ({} records, {} of §8.3 bytes; edge pool {})",
            nav_nodes.len(),
            nav_nodes.len(),
            human(node_b),
            human(edge_b)
        );
        // Fold the edge pool into each junction's weight so one binning pass covers the section.
        let per_node_edge = if nav_nodes.is_empty() { 0.0 } else { edge_b / nav_nodes.len() as f64 };
        for n in &mut nav_nodes {
            n.2 += per_node_edge;
        }
    }

    for &log2 in &opts.cell_log2 {
        let cell: i64 = 1 << log2;
        println!(
            "\n-- cell size 2^{log2} µdeg ({:.4}° ≈ {:.0} km at 47°N) --",
            cell as f64 / 1e6,
            cell as f64 / 1e6 * 111.32 * 0.68
        );
        let mut all: CellBytes = HashMap::new();
        for (i, per_lod) in leaves.iter().enumerate() {
            let mut cells: CellBytes = HashMap::new();
            for (node, b) in per_lod {
                distribute(node, *b, cell, opts.grid_origin, &mut cells);
                distribute(node, *b, cell, opts.grid_origin, &mut all);
            }
            report_cells(&format!("LOD {i}"), &cells);
        }
        report_cells("geometry", &all);
        if !nav_nodes.is_empty() {
            let mut cells: CellBytes = HashMap::new();
            for (lat, lon, b) in &nav_nodes {
                let i = (*lat as i64 - opts.grid_origin).div_euclid(cell);
                let j = (*lon as i64 - opts.grid_origin).div_euclid(cell);
                *cells.entry((i, j)).or_insert(0.0) += b;
            }
            report_cells("nav", &cells);
        }
    }
    println!();
    Ok(())
}

fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    for path in &opts.paths {
        if let Err(e) = survey(path, &opts) {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
