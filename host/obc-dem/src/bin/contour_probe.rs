//! # THROWAWAY — design probe for issue #1078 (EL10 contour lines). NOT PRODUCTION CODE.
//!
//! This binary exists to put **numbers and pictures** in front of a design decision and will be
//! **deleted and rewritten** by the EL10 implementation issue. It is optimised for measurement,
//! not for polish: it allocates freely, holds the whole sample grid in RAM, uses `HashMap` where
//! the real thing must not, and has no tests. Do not build on it.
//!
//! What it does: marching squares over an OBCT terrain shard's sample lattice, chained into
//! polylines, Douglas–Peucker-simplified at each LOD's tolerance the way `obc-pack`'s pipeline
//! simplifies lines (`simplify_m / M_PER_DEG`), then either
//!
//! * `--stats` — reports feature/vertex counts and an OBCM §5.2 byte estimate per LOD, or
//! * `--osm <f.osm>` — writes the contours as OSM XML ways so `osmium` can turn them into a
//!   `.osm.pbf` that `obc-pack` will merge alongside a real extract. That is the mockup seam, and
//!   it is the reason this is throwaway: the real implementation traces inside the packer (or the
//!   bakery), never through OSM XML.
//!
//! ```text
//! obc-dem-contour-probe --terrain grimsel.obcd --interval 50 --stats
//! obc-dem-contour-probe --terrain grimsel.obcd --interval 50 --lod 4 --osm contours.osm
//! ```

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use obc_elevation::grid::lattice_coord;
use obc_elevation::{TerrainReader, TileCache};
use obc_formats::io::SliceSource;

/// The packer's meters→degrees divisor (`obc_map_scene::M_PER_DEG`), duplicated because this
/// throwaway must not pull a firmware render crate in just to read one constant.
const M_PER_DEG: f64 = 111_320.0;

/// The shipped ladder from `builder/presets/schema.json` — (max_mpp, simplify_m).
const LADDER: [(f32, f64); 7] =
    [(f32::INFINITY, 200.0), (30.0, 100.0), (16.0, 40.0), (10.0, 15.0), (5.0, 8.0), (3.0, 3.0), (1.2, 0.5)];

/// OBCM §5.2: the packer densifies any segment longer than this so no delta leaves `int16`.
const DENSIFY_UDEG: i64 = 30_000;
/// OBCM §5.2 per-feature vertex cap (`MAX_FEAT_PTS`).
const MAX_FEAT_PTS: usize = 2048;

// ---------------------------------------------------------------------------------------------
// the sample grid
// ---------------------------------------------------------------------------------------------

/// A dense lattice window read out of an OBCT shard. `NODATA` becomes `None`.
struct Grid {
    /// Lattice index of row 0 / col 0.
    i0: u32,
    j0: u32,
    rows: usize,
    cols: usize,
    posting_log2: u8,
    /// Row-major, `rows * cols`.
    z: Vec<Option<i16>>,
}

impl Grid {
    #[inline]
    fn at(&self, r: usize, c: usize) -> Option<i16> {
        self.z[r * self.cols + c]
    }
    /// µdeg longitude of column `c`.
    #[inline]
    fn lon(&self, c: usize) -> i32 {
        lattice_coord(self.j0 + c as u32, self.posting_log2)
    }
    /// µdeg latitude of row `r`.
    #[inline]
    fn lat(&self, r: usize) -> i32 {
        lattice_coord(self.i0 + r as u32, self.posting_log2)
    }
    #[inline]
    fn posting(&self) -> i32 {
        1i32 << self.posting_log2
    }
}

/// Read the shard's whole cell rectangle into a dense grid.
///
/// Sampling goes through `TerrainReader::sample` at exact lattice coordinates, where OBCT §5's
/// bilinear collapses to the stored sample — so this reuses the shared reader rather than being a
/// second decoder of the container (the §5 "two implementations MUST agree" rule, honoured even by
/// a throwaway).
fn read_grid(reader: &TerrainReader<'_>) -> Grid {
    let h = *reader.header();
    let samples_per_cell = 1u32 << (h.cell_log2 - h.posting_log2);
    let i0 = h.cell_min_i * samples_per_cell;
    let j0 = h.cell_min_j * samples_per_cell;
    let rows = (h.cell_rows as u32 * samples_per_cell) as usize;
    let cols = (h.cell_cols as u32 * samples_per_cell) as usize;

    let mut cache: TileCache<64> = TileCache::new();
    let mut z = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        let lat = lattice_coord(i0 + r as u32, h.posting_log2);
        for c in 0..cols {
            let lon = lattice_coord(j0 + c as u32, h.posting_log2);
            z.push(reader.sample(&mut cache, lat, lon));
        }
    }
    Grid { i0, j0, rows, cols, posting_log2: h.posting_log2, z }
}

// ---------------------------------------------------------------------------------------------
// marching squares
// ---------------------------------------------------------------------------------------------

/// An edge of the sample lattice, keyed so that the two cells sharing it agree by construction.
/// `kind` 0 = horizontal (between `(r,c)` and `(r,c+1)`), 1 = vertical (between `(r,c)`/`(r+1,c)`).
#[inline]
fn edge_key(kind: u8, r: usize, c: usize, cols: usize) -> u64 {
    ((r * (cols + 1) + c) as u64) * 2 + kind as u64
}

/// Trace every closed/open contour of `grid` at `level` metres.
///
/// Returns polylines in µdeg `(lon, lat)`. Cells touching `NODATA` contribute nothing — OBCT
/// principle 6, "a hole is silence": a contour is never drawn across unknown ground.
fn march(grid: &Grid, level: i16) -> Vec<Vec<(i32, i32)>> {
    let lv = level as f64;
    let cols = grid.cols;
    // Crossing point per edge, computed once so both neighbouring cells reuse the same coordinate.
    let mut pts: HashMap<u64, (i32, i32)> = HashMap::new();
    // Chains, as pairs of edge keys.
    let mut segs: Vec<(u64, u64)> = Vec::new();

    let p = grid.posting() as f64;

    // Linear interpolation of the crossing along an edge, in the edge's varying axis.
    let frac = |a: i16, b: i16| -> f64 {
        let (a, b) = (a as f64, b as f64);
        if (b - a).abs() < f64::EPSILON {
            0.0
        } else {
            ((lv - a) / (b - a)).clamp(0.0, 1.0)
        }
    };

    for r in 0..grid.rows.saturating_sub(1) {
        for c in 0..cols.saturating_sub(1) {
            let (Some(a), Some(b), Some(d), Some(e)) =
                (grid.at(r, c), grid.at(r, c + 1), grid.at(r + 1, c + 1), grid.at(r + 1, c))
            else {
                continue; // a NODATA corner voids the cell
            };

            // bit0 = SW(a), bit1 = SE(b), bit2 = NE(d), bit3 = NW(e)
            let case =
                (a > level) as u8 | ((b > level) as u8) << 1 | ((d > level) as u8) << 2 | ((e > level) as u8) << 3;
            if case == 0 || case == 15 {
                continue;
            }

            let south = edge_key(0, r, c, cols);
            let north = edge_key(0, r + 1, c, cols);
            let west = edge_key(1, r, c, cols);
            let east = edge_key(1, r, c + 1, cols);

            // Materialise only the crossings this case actually uses.
            let mut place = |k: u64, kind: u8| {
                pts.entry(k).or_insert_with(|| match kind {
                    // horizontal edge at row rr, varying longitude
                    0 => {
                        let rr = if k == south { r } else { r + 1 };
                        let (l, rgt) = if rr == r { (a, b) } else { (e, d) };
                        (grid.lon(c) + (frac(l, rgt) * p).round() as i32, grid.lat(rr))
                    }
                    // vertical edge at column cc, varying latitude
                    _ => {
                        let cc = if k == west { c } else { c + 1 };
                        let (bot, top) = if cc == c { (a, e) } else { (b, d) };
                        (grid.lon(cc), grid.lat(r) + (frac(bot, top) * p).round() as i32)
                    }
                });
            };

            // Saddle resolution by the cell's mean — the standard disambiguation.
            let centre = (a as f64 + b as f64 + d as f64 + e as f64) / 4.0;
            let centre_above = centre > lv;

            let pairs: &[(u64, u8, u64, u8)] = &match case {
                1 => vec![(west, 1, south, 0)],
                2 => vec![(south, 0, east, 1)],
                3 => vec![(west, 1, east, 1)],
                4 => vec![(east, 1, north, 0)],
                5 => {
                    if centre_above {
                        vec![(west, 1, north, 0), (south, 0, east, 1)]
                    } else {
                        vec![(west, 1, south, 0), (east, 1, north, 0)]
                    }
                }
                6 => vec![(south, 0, north, 0)],
                7 => vec![(west, 1, north, 0)],
                8 => vec![(north, 0, west, 1)],
                9 => vec![(north, 0, south, 0)],
                10 => {
                    if centre_above {
                        vec![(west, 1, south, 0), (east, 1, north, 0)]
                    } else {
                        vec![(west, 1, north, 0), (south, 0, east, 1)]
                    }
                }
                11 => vec![(north, 0, east, 1)],
                12 => vec![(east, 1, west, 1)],
                13 => vec![(east, 1, south, 0)],
                14 => vec![(south, 0, west, 1)],
                _ => vec![],
            };

            for &(k1, t1, k2, t2) in pairs {
                place(k1, t1);
                place(k2, t2);
                segs.push((k1, k2));
            }
        }
    }

    chain(&segs, &pts)
}

/// Chain marching-squares segments into the longest possible polylines.
///
/// Open chains (a contour running off the coverage edge) are walked from their free end; whatever
/// remains is a closed loop and is emitted with its first point repeated at the end.
fn chain(segs: &[(u64, u64)], pts: &HashMap<u64, (i32, i32)>) -> Vec<Vec<(i32, i32)>> {
    let mut adj: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        adj.entry(a).or_default().push(i);
        adj.entry(b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();

    // Walk from `start` along unused segments, returning the edge-key path.
    let walk = |start: u64, used: &mut Vec<bool>| -> Vec<u64> {
        let mut path = vec![start];
        let mut cur = start;
        while let Some(next) = adj.get(&cur).and_then(|v| v.iter().copied().find(|&i| !used[i])) {
            used[next] = true;
            let (a, b) = segs[next];
            cur = if a == cur { b } else { a };
            path.push(cur);
        }
        path
    };

    // Open ends first, so a contour that leaves the box is one feature and not two.
    let mut ends: Vec<u64> = adj.iter().filter(|(_, v)| v.len() == 1).map(|(k, _)| *k).collect();
    ends.sort_unstable(); // determinism: HashMap order must never reach the output
    for e in ends {
        if adj[&e].iter().all(|&i| used[i]) {
            continue;
        }
        let path = walk(e, &mut used);
        if path.len() >= 2 {
            out.push(path.iter().map(|k| pts[k]).collect());
        }
    }
    // Whatever is left is a loop.
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by_key(|&i| segs[i].0);
    for i in order {
        if used[i] {
            continue;
        }
        used[i] = true;
        let (a, b) = segs[i];
        let mut path = walk(b, &mut used);
        path.insert(0, a);
        if path.len() >= 3 {
            out.push(path.iter().map(|k| pts[k]).collect());
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// simplify + byte model
// ---------------------------------------------------------------------------------------------

/// Douglas–Peucker in µdeg space with a µdeg tolerance — the same anisotropic degree-space
/// treatment `obc-pack` gives lines (`tol = simplify_m / M_PER_DEG`), so the counts are comparable.
fn dp(pts: &[(i32, i32)], tol: f64) -> Vec<(i32, i32)> {
    if pts.len() < 3 || tol <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((lo, hi)) = stack.pop() {
        if hi <= lo + 1 {
            continue;
        }
        let (x0, y0) = (pts[lo].0 as f64, pts[lo].1 as f64);
        let (x1, y1) = (pts[hi].0 as f64, pts[hi].1 as f64);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let norm = (dx * dx + dy * dy).sqrt();
        let (mut best, mut bi) = (0.0f64, lo);
        for (i, &(px, py)) in pts.iter().enumerate().take(hi).skip(lo + 1) {
            let (px, py) = (px as f64, py as f64);
            let d = if norm < f64::EPSILON {
                ((px - x0).powi(2) + (py - y0).powi(2)).sqrt()
            } else {
                ((x1 - x0) * (y0 - py) - (x0 - px) * (y1 - y0)).abs() / norm
            };
            if d > best {
                best = d;
                bi = i;
            }
        }
        if best > tol {
            keep[bi] = true;
            stack.push((lo, bi));
            stack.push((bi, hi));
        }
    }
    pts.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect()
}

#[derive(Default, Clone, Copy)]
struct Cost {
    features: usize,
    vertices: usize,
    bytes: usize,
}

/// OBCM §5.2 byte model for a set of polylines at one LOD.
///
/// Densifies segments past `DENSIFY_UDEG` and splits at `MAX_FEAT_PTS` exactly as the packer does,
/// then charges each resulting feature its real header (compact vs wide by vertex count) plus
/// `2` or `4` bytes per delta by the widest delta in the chain. This is the *geometry* cost; the
/// per-leaf split the quadtree adds on top is what the ground-truth pack measures.
fn cost(lines: &[Vec<(i32, i32)>]) -> Cost {
    let mut out = Cost::default();
    for line in lines {
        // densify
        let mut dense: Vec<(i32, i32)> = Vec::with_capacity(line.len());
        for (i, &(x, y)) in line.iter().enumerate() {
            if i > 0 {
                let (px, py) = line[i - 1];
                let steps = (((x as i64 - px as i64).abs().max((y as i64 - py as i64).abs())) / DENSIFY_UDEG) as usize;
                for s in 1..=steps {
                    let t = s as f64 / (steps + 1) as f64;
                    dense.push((
                        px + ((x as i64 - px as i64) as f64 * t) as i32,
                        py + ((y as i64 - py as i64) as f64 * t) as i32,
                    ));
                }
            }
            dense.push((x, y));
        }
        for part in dense.chunks(MAX_FEAT_PTS) {
            if part.len() < 2 {
                continue;
            }
            let wide_deltas = part
                .windows(2)
                .any(|w| (w[1].0 - w[0].0).unsigned_abs() > 127 || (w[1].1 - w[0].1).unsigned_abs() > 127);
            let header = if part.len() > 255 { 12 } else { 7 };
            out.features += 1;
            out.vertices += part.len();
            out.bytes += header + (part.len() - 1) * if wide_deltas { 4 } else { 2 };
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// OSM XML emission (the mockup seam)
// ---------------------------------------------------------------------------------------------

/// The three contour classes one traced PBF carries, so a single mockup bake can serve every style
/// option by changing only the packer config: a class with no `features` rule is simply not packed.
///
/// At `--interval 50 --index-every 10`: `minor` = the 50 m lines, `major` = the 100 m lines,
/// `index` = the 500 m lines.
fn class_of(ele: i32, interval: i32, index_every: i32) -> &'static str {
    if index_every > 0 && ele.rem_euclid(interval * index_every) == 0 {
        "index"
    } else if ele.rem_euclid(interval * 2) == 0 {
        "major"
    } else {
        "minor"
    }
}

/// Write contours as OSM XML ways, tagged `obc_contour=minor|major|index` plus `ele=<m>`, so a
/// packer config styles them with an ordinary `features` rule and **nothing in `obc-pack` changes**.
///
/// IDs start absurdly high so a merge with a real extract cannot collide.
fn write_osm(lines: &[(i16, Vec<(i32, i32)>)], index_every: i32, interval: i32, path: &PathBuf) -> std::io::Result<()> {
    let mut s = String::with_capacity(1 << 25);
    s.push_str("<?xml version='1.0' encoding='UTF-8'?>\n<osm version='0.6' generator='obc-dem contour_probe (#1078 THROWAWAY)'>\n");
    let mut nid: i64 = 50_000_000_000;
    let mut bodies = String::with_capacity(1 << 24);
    for (wid, (ele, line)) in (50_000_000_000_i64..).zip(lines.iter()) {
        let first = nid;
        for &(lon, lat) in line {
            let _ = writeln!(
                s,
                "  <node id='{nid}' version='1' lat='{:.7}' lon='{:.7}'/>",
                lat as f64 / 1e6,
                lon as f64 / 1e6
            );
            nid += 1;
        }
        let _ = writeln!(bodies, "  <way id='{wid}' version='1'>");
        for k in first..nid {
            let _ = writeln!(bodies, "    <nd ref='{k}'/>");
        }
        let _ = writeln!(bodies, "    <tag k='obc_contour' v='{}'/>", class_of(*ele as i32, interval, index_every));
        let _ = writeln!(bodies, "    <tag k='ele' v='{ele}'/>\n  </way>");
    }
    s.push_str(&bodies);
    s.push_str("</osm>\n");
    std::fs::write(path, s)
}

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------

const USAGE: &str = "\
THROWAWAY design probe for #1078 — contour tracing over OBCT.

usage:
  contour_probe --terrain <f.obcd> --interval <m> [--index-every <n>] [--stats]
                [--lod <i>] [--osm <out.osm>] [--min-vertices <n>]

  --stats          per-LOD feature/vertex/byte table (OBCM 5.2 model)
  --lod <i>        simplify at ladder LOD <i> only (for --osm); default 4
  --osm <path>     write OSM XML ways for the mockup repack
  --index-every N  every Nth contour is an index contour (default 5)
  --min-vertices N drop traced lines shorter than N vertices (default 3)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("contour_probe: {e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (mut terrain, mut interval, mut osm, mut lod, mut stats) =
        (None::<PathBuf>, 50i32, None::<PathBuf>, 4usize, false);
    let (mut index_every, mut min_vertices) = (5i32, 3usize);
    let mut it = args.iter();
    while let Some(f) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{f} needs a value"));
        match f.as_str() {
            "--terrain" => terrain = Some(val()?.into()),
            "--interval" => interval = val()?.parse().map_err(|_| "--interval needs metres")?,
            "--index-every" => index_every = val()?.parse().map_err(|_| "--index-every needs a count")?,
            "--min-vertices" => min_vertices = val()?.parse().map_err(|_| "--min-vertices needs a count")?,
            "--lod" => lod = val()?.parse().map_err(|_| "--lod needs an index")?,
            "--osm" => osm = Some(val()?.into()),
            "--stats" => stats = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    let terrain = terrain.ok_or("missing --terrain")?;
    if interval <= 0 {
        return Err("--interval must be positive".into());
    }
    if lod >= LADDER.len() {
        return Err(format!("--lod must be 0..{}", LADDER.len()));
    }

    let bytes = std::fs::read(&terrain).map_err(|e| format!("{}: {e}", terrain.display()))?;
    let src = SliceSource(&bytes);
    let reader = TerrainReader::parse(&src).map_err(|e| format!("{}: {e:?}", terrain.display()))?;

    let t_grid = std::time::Instant::now();
    let grid = read_grid(&reader);
    let grid_ms = t_grid.elapsed().as_secs_f64() * 1e3;

    // Elevation range drives which levels exist at all.
    let (lo, hi) = grid.z.iter().flatten().fold((i16::MAX, i16::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    if lo > hi {
        return Err("terrain is entirely NODATA".into());
    }
    let known = grid.z.iter().filter(|v| v.is_some()).count();

    eprintln!(
        "{}: {}x{} samples ({} known, {:.1}% coverage), {} .. {} m, read in {:.0} ms",
        terrain.display(),
        grid.rows,
        grid.cols,
        known,
        100.0 * known as f64 / grid.z.len() as f64,
        lo,
        hi,
        grid_ms
    );

    // ---- trace every level ----
    let t0 = std::time::Instant::now();
    let first = (lo as i32).div_euclid(interval) * interval;
    let mut traced: Vec<(i16, Vec<(i32, i32)>)> = Vec::new();
    let mut level = first;
    while level <= hi as i32 {
        if level >= lo as i32 {
            for line in march(&grid, level as i16) {
                if line.len() >= min_vertices {
                    traced.push((level as i16, line));
                }
            }
        }
        level += interval;
    }
    let trace_ms = t0.elapsed().as_secs_f64() * 1e3;
    let raw_vertices: usize = traced.iter().map(|(_, l)| l.len()).sum();
    eprintln!(
        "traced {} m interval: {} levels, {} polylines, {} vertices, {:.0} ms",
        interval,
        (hi as i32 - first) / interval + 1,
        traced.len(),
        raw_vertices,
        trace_ms
    );

    if stats {
        println!("\n  LOD  simplify   features   vertices     bytes    KiB   note");
        println!("  ---  --------   --------   --------   -------   ----   ----");
        for (i, (max_mpp, simp)) in LADDER.iter().enumerate() {
            let t1 = std::time::Instant::now();
            let tol = simp / M_PER_DEG * 1e6;
            let simplified: Vec<Vec<(i32, i32)>> =
                traced.iter().map(|(_, l)| dp(l, tol)).filter(|l| l.len() >= 2).collect();
            let c = cost(&simplified);
            let ms = t1.elapsed().as_secs_f64() * 1e3;
            let mpp = if max_mpp.is_finite() { format!("{max_mpp}") } else { "inf".into() };
            println!(
                "  {i:>3}  {simp:>6.1}m   {:>8}   {:>8}   {:>7}   {:>4.0}   <= {mpp} m/px, simplify {ms:.0} ms",
                c.features,
                c.vertices,
                c.bytes,
                c.bytes as f64 / 1024.0
            );
        }
    }

    if let Some(path) = osm {
        let tol = LADDER[lod].1 / M_PER_DEG * 1e6;
        let simplified: Vec<(i16, Vec<(i32, i32)>)> =
            traced.iter().map(|(e, l)| (*e, dp(l, tol))).filter(|(_, l)| l.len() >= 2).collect();
        let v: usize = simplified.iter().map(|(_, l)| l.len()).sum();
        write_osm(&simplified, index_every, interval, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!(
            "wrote {} ({} ways, {} nodes, simplified at LOD {lod} = {}m)",
            path.display(),
            simplified.len(),
            v,
            LADDER[lod].1
        );
    }
    Ok(())
}
