//! **S0 spike (epic #1016)** — can two *independently packed* `.obcm` maps have their §8
//! navigation graphs merged into one routable graph by **coordinate-based node unification**?
//!
//! The cell-catalog epic assembles a map out of pre-baked cells, and the one part of the
//! assembly that cannot be a byte graft is the nav section: node ids are dense and file-local,
//! edge ids are pool byte offsets, and every cell edge is a seam. The epic's hypothesis is that
//! seams unify **by exact µdeg coordinate** (the OSRM/Valhalla tiling trick). This spike tests
//! that hypothesis on the artifacts we can build *today* — two neighbouring Geofabrik extracts
//! packed separately — because a country border is the harshest version of the same seam: no
//! deterministic cut line, no shared pack run, only whatever the two extracts happen to agree on.
//!
//! It is a **spike**: an example, not a shipped tool. It reads finished `.obcm` files through the
//! real `obc-reader`, builds an in-memory merged graph, and prints a report. Nothing here is a
//! library API and no shipped crate behaviour is touched.
//!
//! ```text
//! cargo run --release --example nav_stitch_spike -- \
//!     freiburg.obcm switzerland.obcm \
//!     --route 47.9990,7.8421:47.3769,8.5417 \
//!     --route 47.559,7.588:47.545,7.620
//! ```
//!
//! What it reports, in order:
//!
//! 1. **Per file** — header bbox, junction count, adjacency-entry count, in-file coordinate
//!    collisions (two junctions on one coord, which coordinate keying would fuse).
//! 2. **Overlap** — the double-covered area (a 0.01° occupancy grid intersected), the
//!    border-band width, and how many junctions inside it match **exactly** across files versus
//!    within 1 m / 10 m / 100 m. This is what settles "does Geofabrik hard-cut ways at the
//!    political border, or keep border-crossing ways complete".
//! 3. **Unified routing** — plain Dijkstra over raw `Cost M` on the coordinate-unified union
//!    graph, plus the same query run over each file's edges *alone* as the control (if the
//!    single-file run already reaches the goal, the merge proved nothing).
//! 4. **Components** — connected components of the merged graph, and how many of them are small
//!    enough that `nav.rs`'s island pruning would have eaten them, split by whether they are
//!    made of nodes from one file or both (epic §3: prune-at-assembly).

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use obc_formats::io::{ByteSource, Error as IoError};
use obc_reader::{ground_dist_m, BBox, MapCache, MapTables, NavNodeRef, Reader};

/// Occupancy-grid cell size in µdeg (0.01° ≈ 1.11 km lat, 0.75 km lon at 47.6°N). Coarse enough
/// that a cell is "covered" by any road at all, fine enough to see a border band's shape.
const OCC_CELL_UDEG: i32 = 10_000;

/// Bucket cell for the near-miss search (0.001° ≈ 111 m lat / 75 m lon at 47.6°N). A 3×3 block
/// around a coord therefore always contains everything within ~75 m of it.
const NEAR_CELL_UDEG: i32 = 1_000;

/// One near-miss: an unmatched junction of the first file, its closest unmatched counterpart in
/// the second, and their separation in meters. Coords are `(lat, lon)` µdeg.
type NearMiss = ((i32, i32), (i32, i32), f64);

/// `nav.rs`'s shipped island-pruning threshold (`DEFAULT_MIN_COMPONENT_EDGES`, and what
/// `builder/presets/default.json` sets): keep the largest component plus every component with at
/// least this many edges. Re-stated here so the report can classify merged components by it.
const PRUNE_MIN_COMPONENT_EDGES: usize = 50;

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut routes: Vec<((f64, f64), (f64, f64))> = Vec::new();
    let mut connect_m = 0.0f64;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--route" => {
                let spec = it.next().expect("--route wants latA,lonA:latB,lonB");
                routes.push(parse_route(&spec));
            }
            // Optional epsilon experiment: after exact unification, join each still-unmatched
            // cross-file pair within this many meters with a connector edge of its own length.
            "--connect-m" => connect_m = it.next().and_then(|s| s.parse().ok()).expect("--connect-m wants meters"),
            other => files.push(other.to_string()),
        }
    }
    assert!(files.len() == 2, "usage: nav_stitch_spike <a.obcm> <b.obcm> [--route lat,lon:lat,lon] [--connect-m M]");
    if routes.is_empty() {
        // Freiburg im Breisgau → Zürich (the long one), plus two short hops that must cross the
        // border and whose endpoints sit clear of either extract's overhang band (~1–3 km):
        // Lörrach (DE) → Liestal (CH), and Waldshut (DE) → Brugg (CH).
        routes.push(((47.9990, 7.8421), (47.3769, 8.5417)));
        routes.push(((47.6150, 7.6600), (47.4840, 7.7350)));
        routes.push(((47.6230, 8.2140), (47.4810, 8.2080)));
    }

    let a = FileGraph::load(&files[0]);
    let b = FileGraph::load(&files[1]);
    a.report();
    b.report();

    let mutual = overlap_report(&a, &b);

    let merged = Merged::build(&a, &b, connect_m);
    merged.report();
    merged.stub_report(&mutual);

    for (from, to) in &routes {
        merged.route_report(*from, *to);
    }

    merged.component_report(&mutual);
}

fn parse_route(spec: &str) -> ((f64, f64), (f64, f64)) {
    let (from, to) = spec.split_once(':').expect("--route wants latA,lonA:latB,lonB");
    (parse_ll(from), parse_ll(to))
}

fn parse_ll(s: &str) -> (f64, f64) {
    let (lat, lon) = s.split_once(',').expect("a point is lat,lon in degrees");
    (lat.trim().parse().expect("lat"), lon.trim().parse().expect("lon"))
}

// --- one file's nav graph, fully resident -----------------------------------------------------

/// One decoded §8.3 junction: absolute µdeg coord, the file-local dense id, and its adjacency.
struct Junction {
    lat: i32,
    lon: i32,
    nbrs: Vec<Adj>,
}

/// One adjacency entry, with the neighbour's reconstructed absolute coord. `way_kind` is dropped:
/// the spike routes on raw `Cost M`, deliberately ignoring profiles.
#[derive(Clone, Copy)]
struct Adj {
    lat: i32,
    lon: i32,
    cost_m: u32,
}

struct FileGraph {
    label: String,
    bbox: BBox,
    /// Junctions keyed by their file-local `Node Id` — the walk can hand the same record back
    /// more than once (§8.2 bin-packing shares chunks between leaves), so collection must be
    /// idempotent, and the id is the identity the spec names for that.
    junctions: HashMap<u32, Junction>,
    /// Occupied 0.01° cells — the file's *data* footprint, which is much tighter than its bbox.
    occ: HashSet<(i32, i32)>,
}

impl FileGraph {
    fn load(path: &str) -> FileGraph {
        let label = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let src = FileSource::open(Path::new(path));
        let tables = MapTables::parse(&src).expect("readable OBCM map");
        // ~277 KB — fine on a host main stack (the `alloc` feature isn't on for this crate's
        // reader dependency, so `new_boxed` isn't available here).
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let bbox = reader.bbox;
        let dir = *reader.nav_directory();
        assert!(!dir.is_empty(), "{label}: no nav graph in this map");

        let mut junctions: HashMap<u32, Junction> = HashMap::new();
        let mut occ: HashSet<(i32, i32)> = HashSet::new();
        let mut scratch = vec![0u8; dir.chunk_size];
        // One walk over the whole header bbox visits every leaf, hence every junction record.
        reader
            .for_each_nav_node(&bbox, &mut scratch, |n: NavNodeRef| {
                if junctions.contains_key(&n.id) {
                    return; // a chunk shared between leaves is decoded once per leaf
                }
                occ.insert(occ_cell(n.lat, n.lon));
                let nbrs = n.neighbors().map(|nb| Adj { lat: nb.lat, lon: nb.lon, cost_m: nb.cost_m }).collect();
                junctions.insert(n.id, Junction { lat: n.lat, lon: n.lon, nbrs });
            })
            .expect("nav node walk");
        FileGraph { label, bbox, junctions, occ }
    }

    fn adj_entries(&self) -> usize {
        self.junctions.values().map(|j| j.nbrs.len()).sum()
    }

    /// Coordinates that carry more than one junction id — coordinate keying fuses these, so the
    /// count is the in-file cost of the unification key itself.
    fn coord_collisions(&self) -> (usize, usize) {
        let mut per_coord: HashMap<(i32, i32), usize> = HashMap::new();
        for j in self.junctions.values() {
            *per_coord.entry((j.lat, j.lon)).or_insert(0) += 1;
        }
        let dup_coords = per_coord.values().filter(|&&c| c > 1).count();
        (per_coord.len(), dup_coords)
    }

    fn report(&self) {
        let (coords, dups) = self.coord_collisions();
        println!("== file {}", self.label);
        println!(
            "   bbox           {:.5},{:.5} .. {:.5},{:.5}  (lat,lon)",
            self.bbox.min_lat as f64 / 1e6,
            self.bbox.min_lon as f64 / 1e6,
            self.bbox.max_lat as f64 / 1e6,
            self.bbox.max_lon as f64 / 1e6
        );
        println!("   junctions      {}", self.junctions.len());
        println!("   adjacency      {} entries (≈ {} undirected edges)", self.adj_entries(), self.adj_entries() / 2);
        println!("   distinct coords {coords}  (coords carrying >1 junction: {dups})");
        println!("   occupied 0.01° cells {}", self.occ.len());
    }
}

fn occ_cell(lat: i32, lon: i32) -> (i32, i32) {
    (lat.div_euclid(OCC_CELL_UDEG), lon.div_euclid(OCC_CELL_UDEG))
}

fn near_cell(lat: i32, lon: i32) -> (i32, i32) {
    (lat.div_euclid(NEAR_CELL_UDEG), lon.div_euclid(NEAR_CELL_UDEG))
}

fn dist_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    // `ground_dist_m` takes (lon, lat) pairs — the crate's geometry order.
    ground_dist_m((a.1, a.0), (b.1, b.0)) as f64
}

// --- overlap / near-miss analysis --------------------------------------------------------------

/// Prints the overlap analysis and returns the mutually covered 0.01° cells (the real
/// double-coverage region, which is far tighter than the bbox intersection).
fn overlap_report(a: &FileGraph, b: &FileGraph) -> HashSet<(i32, i32)> {
    println!("\n== overlap");
    let bbox_lat = (a.bbox.min_lat.max(b.bbox.min_lat), a.bbox.max_lat.min(b.bbox.max_lat));
    let bbox_lon = (a.bbox.min_lon.max(b.bbox.min_lon), a.bbox.max_lon.min(b.bbox.max_lon));
    println!(
        "   bbox intersection {:.4},{:.4} .. {:.4},{:.4} (lat,lon) — a bbox band, NOT the data overlap",
        bbox_lat.0 as f64 / 1e6,
        bbox_lon.0 as f64 / 1e6,
        bbox_lat.1 as f64 / 1e6,
        bbox_lon.1 as f64 / 1e6
    );

    // The real double-coverage region: cells where *both* files have junctions.
    let mutual: HashSet<(i32, i32)> = a.occ.intersection(&b.occ).copied().collect();
    let cell_km2 = (OCC_CELL_UDEG as f64 / 1e6) * 111.32 * (OCC_CELL_UDEG as f64 / 1e6) * 111.32 * 0.674;
    println!("   mutually covered 0.01° cells {} (≈ {:.0} km²)", mutual.len(), mutual.len() as f64 * cell_km2);

    // Band shape: per lon column, how many latitude cells deep is the double coverage?
    let mut per_col: HashMap<i32, (i32, i32, usize)> = HashMap::new();
    for &(clat, clon) in &mutual {
        let e = per_col.entry(clon).or_insert((clat, clat, 0));
        e.0 = e.0.min(clat);
        e.1 = e.1.max(clat);
        e.2 += 1;
    }
    let mut widths: Vec<f64> =
        per_col.values().map(|&(lo, hi, _)| (hi - lo + 1) as f64 * OCC_CELL_UDEG as f64 / 1e6 * 111.32).collect();
    widths.sort_by(|x, y| x.partial_cmp(y).unwrap());
    if !widths.is_empty() {
        let pct = |p: f64| widths[((widths.len() - 1) as f64 * p) as usize];
        println!(
            "   band depth over {} lon columns: min {:.1} km, p50 {:.1} km, p90 {:.1} km, max {:.1} km",
            widths.len(),
            widths[0],
            pct(0.5),
            pct(0.9),
            widths[widths.len() - 1]
        );
    }

    // Match analysis, restricted to the mutually covered cells. Two rings: the whole mutual
    // region, and its *interior* (every 8-neighbour also mutual) — the interior is where both
    // extracts really do describe the same ground, so a match rate below 100% there would mean
    // coordinate unification is not enough on its own.
    let interior: HashSet<(i32, i32)> = mutual
        .iter()
        .copied()
        .filter(|&(clat, clon)| (-1..=1).all(|dlat| (-1..=1).all(|dlon| mutual.contains(&(clat + dlat, clon + dlon)))))
        .collect();
    println!("   interior cells (all 8 neighbours mutual too) {}", interior.len());

    let coords_a: HashSet<(i32, i32)> = a.junctions.values().map(|j| (j.lat, j.lon)).collect();
    let coords_b: HashSet<(i32, i32)> = b.junctions.values().map(|j| (j.lat, j.lon)).collect();

    for (name, region) in [("mutual", &mutual), ("interior", &interior)] {
        let in_region = |lat: i32, lon: i32| region.contains(&occ_cell(lat, lon));
        let band_a: Vec<(i32, i32)> = coords_a.iter().copied().filter(|&(lat, lon)| in_region(lat, lon)).collect();
        let band_b: Vec<(i32, i32)> = coords_b.iter().copied().filter(|&(lat, lon)| in_region(lat, lon)).collect();
        let exact = band_a.iter().filter(|c| coords_b.contains(c)).count();
        println!(
            "   [{name}] junctions in region: {} ({}) / {} ({}) — exact coord matches {exact} \
             ({:.1}% of {}, {:.1}% of {})",
            band_a.len(),
            a.label,
            band_b.len(),
            b.label,
            100.0 * exact as f64 / band_a.len().max(1) as f64,
            a.label,
            100.0 * exact as f64 / band_b.len().max(1) as f64,
            b.label
        );

        // Near misses: for every unmatched A-junction in the region, the nearest unmatched
        // B-junction. Bucketed, because "needs an epsilon snap" is exactly the question.
        let unmatched_b: Vec<(i32, i32)> = band_b.iter().copied().filter(|c| !coords_a.contains(c)).collect();
        let mut grid: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        for &c in &unmatched_b {
            grid.entry(near_cell(c.0, c.1)).or_default().push(c);
        }
        // Buckets: ≤1 m, ≤10 m, ≤100 m, ≤1000 m, farther/none. The 3×3 block guarantees the
        // search sees everything within ~75 m (0.001° of lon at 47.6°N); the wider buckets are
        // best-effort within that block and only there to show the shape of the tail.
        let mut buckets = [0usize; 5];
        let mut closest: Vec<NearMiss> = Vec::new();
        for &c in band_a.iter().filter(|c| !coords_b.contains(c)) {
            let (cl, cn) = near_cell(c.0, c.1);
            let mut best: Option<((i32, i32), f64)> = None;
            for dlat in -1..=1 {
                for dlon in -1..=1 {
                    for &o in grid.get(&(cl + dlat, cn + dlon)).into_iter().flatten() {
                        let d = dist_m(c, o);
                        if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                            best = Some((o, d));
                        }
                    }
                }
            }
            match best {
                Some((_, d)) if d <= 1.0 => buckets[0] += 1,
                Some((_, d)) if d <= 10.0 => buckets[1] += 1,
                Some((_, d)) if d <= 100.0 => buckets[2] += 1,
                Some((_, d)) if d <= 1000.0 => buckets[3] += 1,
                _ => buckets[4] += 1,
            }
            if let Some((o, d)) = best {
                closest.push((c, o, d));
            }
        }
        closest.sort_by(|x, y| x.2.partial_cmp(&y.2).unwrap());
        println!(
            "   [{name}] unmatched {} junctions: nearest unmatched {} junction ≤1 m {}, ≤10 m {}, ≤100 m {}, \
             ≤1000 m {}, farther/none {}",
            band_a.iter().filter(|c| !coords_b.contains(c)).count(),
            b.label,
            buckets[0],
            buckets[1],
            buckets[2],
            buckets[3],
            buckets[4]
        );
        for (c, o, d) in closest.iter().take(5) {
            println!(
                "      closest unmatched pair: {} ({:.6},{:.6}) vs {} ({:.6},{:.6}) — {:.2} m",
                a.label,
                c.0 as f64 / 1e6,
                c.1 as f64 / 1e6,
                b.label,
                o.0 as f64 / 1e6,
                o.1 as f64 / 1e6,
                d
            );
        }
    }

    // A handful of concrete exact matches, for eyeballing on a map.
    let mut shared: Vec<(i32, i32)> = coords_a.intersection(&coords_b).copied().collect();
    shared.sort();
    println!("   total exact coord matches anywhere: {}", shared.len());
    for c in shared.iter().step_by((shared.len() / 6).max(1)).take(6) {
        println!("      shared junction {:.6},{:.6}", c.0 as f64 / 1e6, c.1 as f64 / 1e6);
    }

    // Transects: at a few sample longitudes, how far does each extract's road data reach past
    // the other's, and where do the shared junctions sit? This is the "does Geofabrik hard-cut
    // border-crossing ways" measurement, read straight off the data.
    println!("   transects (0.01°-wide lon columns): each file's data extent + the shared band");
    let lon_lo = bbox_lon.0.div_euclid(OCC_CELL_UDEG);
    let lon_hi = bbox_lon.1.div_euclid(OCC_CELL_UDEG);
    let step = ((lon_hi - lon_lo) / 9).max(1);
    let shared_set: HashSet<(i32, i32)> = shared.iter().copied().collect();
    for clon in (lon_lo..=lon_hi).step_by(step as usize) {
        let col = |set: &HashSet<(i32, i32)>| {
            let lats: Vec<i32> = set.iter().filter(|c| c.1.div_euclid(OCC_CELL_UDEG) == clon).map(|c| c.0).collect();
            if lats.is_empty() {
                None
            } else {
                Some((*lats.iter().min().unwrap(), *lats.iter().max().unwrap(), lats.len()))
            }
        };
        let (ca, cb, cs) = (col(&coords_a), col(&coords_b), col(&shared_set));
        let fmt = |c: Option<(i32, i32, usize)>| match c {
            Some((lo, hi, n)) => format!("{:.4}..{:.4} n={n}", lo as f64 / 1e6, hi as f64 / 1e6),
            None => "—".to_string(),
        };
        let overlap_km = match (ca, cb) {
            (Some((alo, _, _)), Some((_, bhi, _))) if bhi > alo => (bhi - alo) as f64 / 1e6 * 111.32,
            _ => 0.0,
        };
        println!(
            "      lon {:.2}: {} lat {} | {} lat {} | shared lat {} | vertical double-cover {:.1} km",
            clon as f64 * OCC_CELL_UDEG as f64 / 1e6,
            a.label,
            fmt(ca),
            b.label,
            fmt(cb),
            fmt(cs),
            overlap_km
        );
    }
    mutual
}

// --- the merged graph -------------------------------------------------------------------------

/// Source-file provenance bits for a merged node or edge.
const FROM_A: u8 = 1;
const FROM_B: u8 = 2;

struct Merged {
    label_a: String,
    label_b: String,
    coords: Vec<(i32, i32)>,
    index: HashMap<(i32, i32), u32>,
    /// Per node: provenance mask.
    from: Vec<u8>,
    /// Per node: `(neighbour, cost_m, provenance mask)`.
    adj: Vec<Vec<(u32, u32, u8)>>,
    edges: usize,
    edges_both: usize,
    connectors: usize,
}

impl Merged {
    fn build(a: &FileGraph, b: &FileGraph, connect_m: f64) -> Merged {
        let mut m = Merged {
            label_a: a.label.clone(),
            label_b: b.label.clone(),
            coords: Vec::new(),
            index: HashMap::new(),
            from: Vec::new(),
            adj: Vec::new(),
            edges: 0,
            edges_both: 0,
            connectors: 0,
        };
        // Union of both node sets, keyed by exact coordinate: a shared coord is ONE node.
        for (g, bit) in [(a, FROM_A), (b, FROM_B)] {
            for j in g.junctions.values() {
                let id = m.node(j.lat, j.lon);
                m.from[id as usize] |= bit;
                for nb in &j.nbrs {
                    let nid = m.node(nb.lat, nb.lon);
                    m.from[nid as usize] |= bit;
                }
            }
        }
        // Edges, deduped on (endpoint pair, cost) — the same road described by both files
        // collapses to one edge, and genuinely distinct parallel edges survive.
        let mut seen: HashSet<(u32, u32, u32)> = HashSet::new();
        for (g, bit) in [(a, FROM_A), (b, FROM_B)] {
            for j in g.junctions.values() {
                let u = m.index[&(j.lat, j.lon)];
                for nb in &j.nbrs {
                    let v = m.index[&(nb.lat, nb.lon)];
                    let key = (u.min(v), u.max(v), nb.cost_m);
                    if seen.insert(key) {
                        m.adj[u as usize].push((v, nb.cost_m, bit));
                        if u != v {
                            m.adj[v as usize].push((u, nb.cost_m, bit));
                        }
                        m.edges += 1;
                    } else {
                        // Already present: the other file describes the same road — stamp it.
                        m.stamp(u, v, nb.cost_m, bit);
                    }
                }
            }
        }
        m.edges_both = m
            .adj
            .iter()
            .enumerate()
            .flat_map(|(u, l)| l.iter().map(move |e| (u as u32, *e)))
            .filter(|(u, (v, _, mask))| u <= v && *mask == (FROM_A | FROM_B))
            .count();

        // Optional epsilon experiment: connect still-unmatched cross-file pairs within
        // `connect_m` meters with a connector edge whose cost is their separation.
        if connect_m > 0.0 {
            let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
            for (i, &(lat, lon)) in m.coords.iter().enumerate() {
                if m.from[i] == FROM_B {
                    grid.entry(near_cell(lat, lon)).or_default().push(i as u32);
                }
            }
            let a_only: Vec<u32> = (0..m.coords.len() as u32).filter(|&i| m.from[i as usize] == FROM_A).collect();
            for u in a_only {
                let (lat, lon) = m.coords[u as usize];
                let (cl, cn) = near_cell(lat, lon);
                let mut best: Option<(u32, f64)> = None;
                for dlat in -1..=1 {
                    for dlon in -1..=1 {
                        for &v in grid.get(&(cl + dlat, cn + dlon)).into_iter().flatten() {
                            let d = dist_m((lat, lon), m.coords[v as usize]);
                            if d <= connect_m && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                                best = Some((v, d));
                            }
                        }
                    }
                }
                if let Some((v, d)) = best {
                    let cost = d.round().max(1.0) as u32;
                    m.adj[u as usize].push((v, cost, FROM_A | FROM_B));
                    m.adj[v as usize].push((u, cost, FROM_A | FROM_B));
                    m.connectors += 1;
                }
            }
        }
        m
    }

    fn node(&mut self, lat: i32, lon: i32) -> u32 {
        match self.index.get(&(lat, lon)) {
            Some(&id) => id,
            None => {
                let id = self.coords.len() as u32;
                self.coords.push((lat, lon));
                self.from.push(0);
                self.adj.push(Vec::new());
                self.index.insert((lat, lon), id);
                id
            }
        }
    }

    /// Add `bit` to an existing edge's provenance in both directions.
    fn stamp(&mut self, u: u32, v: u32, cost: u32, bit: u8) {
        for (x, y) in [(u, v), (v, u)] {
            for e in self.adj[x as usize].iter_mut() {
                if e.0 == y && e.1 == cost {
                    e.2 |= bit;
                }
            }
        }
    }

    fn report(&self) {
        let both = self.from.iter().filter(|&&f| f == (FROM_A | FROM_B)).count();
        println!("\n== merged graph (exact-coordinate unification)");
        println!("   nodes {} (unified/shared by both files: {both})", self.coords.len());
        println!("   undirected edges {} (described by both files: {})", self.edges, self.edges_both);
        if self.connectors > 0 {
            println!("   epsilon connector edges added: {}", self.connectors);
        }

        // Do the §8.3 wire limits survive unification? Degree cap 24, `int16` neighbour deltas.
        let mut max_deg = 0usize;
        let mut over_cap = 0usize;
        let mut max_delta = 0i64;
        let mut union_nodes = 0usize; // shared nodes whose adjacency is genuinely a union
        for (i, list) in self.adj.iter().enumerate() {
            max_deg = max_deg.max(list.len());
            over_cap += usize::from(list.len() > 24);
            let (lat, lon) = self.coords[i];
            let (mut da, mut db) = (0usize, 0usize);
            for &(v, _, mask) in list {
                let (nlat, nlon) = self.coords[v as usize];
                max_delta = max_delta.max((nlat as i64 - lat as i64).abs()).max((nlon as i64 - lon as i64).abs());
                da += usize::from(mask & FROM_A != 0);
                db += usize::from(mask & FROM_B != 0);
            }
            if da > 0 && db > 0 && list.len() > da.max(db) {
                union_nodes += 1;
            }
        }
        println!("   max merged degree {max_deg} (nodes over the §8.3 cap of 24: {over_cap})");
        println!("   max neighbour delta {max_delta} µdeg (i16 bound 32767; nav.rs splits at 32000)");
        println!("   nodes whose adjacency is a genuine union of both files' entries: {union_nodes}");
    }

    /// Dead-end stubs: a way that one extract cut mid-road leaves a degree-1 junction behind. If
    /// the two extracts hard-cut at the political border, the double-covered band is full of
    /// them and unification has nothing to join; if they keep border-crossing ways complete, the
    /// band's degree-1 rate looks like anywhere else. `mutual` is the double-coverage region.
    fn stub_report(&self, mutual: &HashSet<(i32, i32)>) {
        let mut in_band = (0usize, 0usize, 0usize); // (nodes, degree-1, degree-1 single-file)
        let mut outside = (0usize, 0usize);
        for (i, &(lat, lon)) in self.coords.iter().enumerate() {
            let deg1 = self.adj[i].len() == 1;
            if mutual.contains(&occ_cell(lat, lon)) {
                in_band.0 += 1;
                in_band.1 += usize::from(deg1);
                in_band.2 += usize::from(deg1 && self.from[i] != (FROM_A | FROM_B));
            } else {
                outside.0 += 1;
                outside.1 += usize::from(deg1);
            }
        }
        println!("   degree-1 (dead-end) rate inside the double-covered band: {}/{} = {:.2}% ({} of them known to only one file)",
            in_band.1, in_band.0, 100.0 * in_band.1 as f64 / in_band.0.max(1) as f64, in_band.2);
        println!(
            "   degree-1 rate outside the band (control): {}/{} = {:.2}%",
            outside.1,
            outside.0,
            100.0 * outside.1 as f64 / outside.0.max(1) as f64
        );
    }

    fn nearest(&self, lat: f64, lon: f64, mask: Option<u8>) -> (u32, f64) {
        let target = ((lat * 1e6) as i32, (lon * 1e6) as i32);
        let mut best = (u32::MAX, f64::MAX);
        for (i, &c) in self.coords.iter().enumerate() {
            if let Some(m) = mask {
                if self.from[i] & m == 0 {
                    continue;
                }
            }
            let d = dist_m(target, c);
            if d < best.1 {
                best = (i as u32, d);
            }
        }
        best
    }

    /// Dijkstra over raw `Cost M`. `only` restricts relaxation to edges a given file describes —
    /// the single-file control runs.
    fn dijkstra(&self, start: u32, goal: u32, only: Option<u8>) -> Option<(u32, Vec<u32>)> {
        let mut dist: HashMap<u32, u32> = HashMap::new();
        let mut prev: HashMap<u32, u32> = HashMap::new();
        let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
        dist.insert(start, 0);
        heap.push(Reverse((0, start)));
        while let Some(Reverse((d, u))) = heap.pop() {
            if u == goal {
                let mut path = vec![goal];
                let mut cur = goal;
                while let Some(&p) = prev.get(&cur) {
                    path.push(p);
                    cur = p;
                }
                path.reverse();
                return Some((d, path));
            }
            if dist.get(&u).copied().unwrap_or(u32::MAX) < d {
                continue;
            }
            for &(v, cost, mask) in &self.adj[u as usize] {
                if let Some(m) = only {
                    if mask & m == 0 {
                        continue;
                    }
                }
                let nd = d.saturating_add(cost);
                if nd < dist.get(&v).copied().unwrap_or(u32::MAX) {
                    dist.insert(v, nd);
                    prev.insert(v, u);
                    heap.push(Reverse((nd, v)));
                }
            }
        }
        None
    }

    fn route_report(&self, from: (f64, f64), to: (f64, f64)) {
        println!("\n== route {:.4},{:.4} → {:.4},{:.4}", from.0, from.1, to.0, to.1);
        let (s, ds) = self.nearest(from.0, from.1, None);
        let (g, dg) = self.nearest(to.0, to.1, None);
        let tag = |i: u32| match self.from[i as usize] {
            3 => "shared",
            2 => self.label_b.as_str(),
            _ => self.label_a.as_str(),
        };
        println!(
            "   start node {:.6},{:.6} ({} m off, {}), goal {:.6},{:.6} ({:.0} m off, {})",
            self.coords[s as usize].0 as f64 / 1e6,
            self.coords[s as usize].1 as f64 / 1e6,
            ds.round(),
            tag(s),
            self.coords[g as usize].0 as f64 / 1e6,
            self.coords[g as usize].1 as f64 / 1e6,
            dg,
            tag(g)
        );

        for (name, only) in
            [(format!("{} alone", self.label_a), Some(FROM_A)), (format!("{} alone", self.label_b), Some(FROM_B))]
        {
            match self.dijkstra(s, g, only) {
                Some((cost, path)) => {
                    println!("   control [{name}]: REACHABLE {:.2} km over {} nodes", cost as f64 / 1000.0, path.len())
                }
                None => println!("   control [{name}]: unreachable (as expected for a cross-border query)"),
            }
        }

        match self.dijkstra(s, g, None) {
            None => println!("   merged: UNREACHABLE — exact-coordinate unification did not stitch this pair"),
            Some((cost, path)) => {
                let shared = path.iter().filter(|&&i| self.from[i as usize] == (FROM_A | FROM_B)).count();
                let a_only = path.iter().filter(|&&i| self.from[i as usize] == FROM_A).count();
                let b_only = path.iter().filter(|&&i| self.from[i as usize] == FROM_B).count();
                println!(
                    "   merged: REACHABLE {:.2} km over {} nodes — {} shared (border-unified), {} {}-only, {} {}-only",
                    cost as f64 / 1000.0,
                    path.len(),
                    shared,
                    a_only,
                    self.label_a,
                    b_only,
                    self.label_b
                );
                // Integrity check: no step may join two nodes that share no file. An edge always
                // comes from one file, so both its endpoints exist in that file — a violation
                // would mean the merge invented connectivity.
                let bad_steps =
                    path.windows(2).filter(|w| self.from[w[0] as usize] & self.from[w[1] as usize] == 0).count();

                // Where does the route hand over from one file to the other? Walk the path
                // ignoring shared nodes (a border-unified run of them is the handover itself)
                // and record every A-only → B-only transition, with the shared nodes it crossed.
                let mut handovers: Vec<(i32, i32)> = Vec::new();
                let mut last_single: Option<u8> = None;
                let mut shared_run: Vec<u32> = Vec::new();
                for &i in &path {
                    match self.from[i as usize] {
                        3 => shared_run.push(i),
                        f => {
                            if let Some(prev) = last_single {
                                if prev != f {
                                    // A transition. The crossing point is the shared run
                                    // between the two single-file stretches (or the step itself).
                                    let at = shared_run.last().or(shared_run.first()).copied().unwrap_or(i);
                                    handovers.push(self.coords[at as usize]);
                                }
                            }
                            last_single = Some(f);
                            shared_run.clear();
                        }
                    }
                }
                println!("      file-to-file handovers on the route: {} (invalid steps: {bad_steps})", handovers.len());
                for c in handovers.iter().take(8) {
                    println!("      handover near {:.6},{:.6}", c.0 as f64 / 1e6, c.1 as f64 / 1e6);
                }
                let shared_coords: Vec<(i32, i32)> =
                    path.iter().filter(|&&i| self.from[i as usize] == 3).map(|&i| self.coords[i as usize]).collect();
                for c in shared_coords.iter().take(6) {
                    println!(
                        "      route uses shared (unified) junction {:.6},{:.6}",
                        c.0 as f64 / 1e6,
                        c.1 as f64 / 1e6
                    );
                }
                // Edge provenance along the route, by length.
                let (mut km_a, mut km_b, mut km_both) = (0.0, 0.0, 0.0);
                for w in path.windows(2) {
                    // Parallel edges are possible; the relaxation used the cheapest one.
                    if let Some(&(_, cost, mask)) =
                        self.adj[w[0] as usize].iter().filter(|e| e.0 == w[1]).min_by_key(|e| e.1)
                    {
                        let km = cost as f64 / 1000.0;
                        match mask {
                            3 => km_both += km,
                            2 => km_b += km,
                            _ => km_a += km,
                        }
                    }
                }
                println!(
                    "      length by edge provenance: {:.1} km {}-only, {:.1} km {}-only, {:.1} km described by both",
                    km_a, self.label_a, km_b, self.label_b, km_both
                );
            }
        }
    }

    /// Connected components of the merged graph, classified against `nav.rs`'s island-pruning
    /// threshold — the epic §3 question of whether pruning can stay at bake time.
    fn component_report(&self, mutual: &HashSet<(i32, i32)>) {
        println!("\n== components of the merged graph (island-pruning semantics)");
        let n = self.coords.len();
        let mut comp = vec![u32::MAX; n];
        let mut sizes: Vec<Comp> = Vec::new();
        for s in 0..n {
            if comp[s] != u32::MAX {
                continue;
            }
            let cid = sizes.len() as u32;
            let mut stack = vec![s];
            comp[s] = cid;
            let mut c = Comp::default();
            while let Some(u) = stack.pop() {
                c.nodes += 1;
                if mutual.contains(&occ_cell(self.coords[u].0, self.coords[u].1)) {
                    c.band_nodes += 1;
                }
                for &(v, _, em) in &self.adj[u] {
                    c.half_edges += 1;
                    // Per-file edge slices: what each file alone would have called this
                    // component. Half-counted like `half_edges`.
                    c.half_a += usize::from(em & FROM_A != 0);
                    c.half_b += usize::from(em & FROM_B != 0);
                    if comp[v as usize] == u32::MAX {
                        comp[v as usize] = cid;
                        stack.push(v as usize);
                    }
                }
            }
            sizes.push(c);
        }
        let largest = sizes.iter().enumerate().max_by_key(|(_, s)| s.nodes).map(|(i, _)| i).unwrap_or(0);
        let small: Vec<&Comp> = sizes
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != largest && s.edges() < PRUNE_MIN_COMPONENT_EDGES)
            .map(|(_, s)| s)
            .collect();
        println!(
            "   components {} (largest {} nodes / {} edges)",
            sizes.len(),
            sizes[largest].nodes,
            sizes[largest].edges()
        );
        println!(
            "   below the shipped prune threshold ({PRUNE_MIN_COMPONENT_EDGES} edges): {} components, {} nodes, {} edges",
            small.len(),
            small.iter().map(|s| s.nodes).sum::<usize>(),
            small.iter().map(|s| s.edges()).sum::<usize>()
        );
        let small_band = small.iter().filter(|s| s.band_nodes > 0).count();
        println!("   ... of those, {small_band} have at least one node inside the double-covered band");
        // The epic §3 loss case: a component big enough to keep once merged, yet small enough in
        // each file on its own that per-file (bake-time) pruning would have dropped both halves.
        let rescued: Vec<&Comp> = sizes
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                *i != largest
                    && s.edges() >= PRUNE_MIN_COMPONENT_EDGES
                    && s.edges_a() < PRUNE_MIN_COMPONENT_EDGES
                    && s.edges_b() < PRUNE_MIN_COMPONENT_EDGES
                    && s.edges_a() > 0
                    && s.edges_b() > 0
            })
            .map(|(_, s)| s)
            .collect();
        println!(
            "   merge-rescued components (≥ threshold merged, < threshold in EACH file alone): {} ({} nodes, {} edges)",
            rescued.len(),
            rescued.iter().map(|s| s.nodes).sum::<usize>(),
            rescued.iter().map(|s| s.edges()).sum::<usize>()
        );
        let mixed_all = sizes.iter().filter(|s| s.edges_a() > 0 && s.edges_b() > 0).count();
        println!("   components described by both files at all: {mixed_all}");
        println!(
            "   largest component holds {:.2}% of all merged nodes",
            100.0 * sizes[largest].nodes as f64 / n as f64
        );
    }
}

/// One connected component of the merged graph. Edge counts are accumulated per half-edge (both
/// directions) and halved on read, so a self-loop counts once like §8.3's does.
#[derive(Default)]
struct Comp {
    nodes: usize,
    band_nodes: usize,
    half_edges: usize,
    half_a: usize,
    half_b: usize,
}

impl Comp {
    fn edges(&self) -> usize {
        self.half_edges / 2
    }
    /// Edges the first file describes — what per-file pruning would have counted.
    fn edges_a(&self) -> usize {
        self.half_a / 2
    }
    fn edges_b(&self) -> usize {
        self.half_b / 2
    }
}

// --- a file-backed ByteSource (the obc-bake verify pattern) ------------------------------------

struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    fn open(path: &Path) -> FileSource {
        let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let len = u32::try_from(file.metadata().expect("metadata").len()).expect("map fits a u32 address space");
        FileSource { file: RefCell::new(file), len }
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let end = (offset as u64).checked_add(buf.len() as u64).ok_or(IoError::BadOffset)?;
        if end > u64::from(self.len) {
            return Err(IoError::BadOffset);
        }
        let mut file = self.file.try_borrow_mut().map_err(|_| IoError::Io)?;
        file.seek(SeekFrom::Start(u64::from(offset))).map_err(|_| IoError::Io)?;
        file.read_exact(buf).map_err(|_| IoError::Io)
    }

    fn len(&self) -> u32 {
        self.len
    }
}
