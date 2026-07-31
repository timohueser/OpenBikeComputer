//! `ingest.rs` — read one or more `.osm.pbf` files into styled features (lines,
//! closed-way polygons, and multipolygon/`boundary` relation areas). Two `osmpbf`
//! passes — three with a `--bbox`, which prepends a **pass 0** (see the cropping
//! section at the end of this doc):
//!
//!   - **Pass 1** builds the `node_id → coord` store and collects qualifying area
//!     relations. Relations sit last in a sorted PBF, so one whole-file read sees
//!     them after the nodes — no extra pass. Tagged nodes are also matched
//!     against the POI table here ([`crate::poi`]).
//!   - **Pass 2** resolves ways into features + coastlines and captures the
//!     geometry of any way that is a relation member. Closed ways matching the
//!     POI table yield centroid POIs.
//!
//! Each relation's member ways are then assembled into polygons-with-holes via
//! [`assemble_multipolygon`]. Assembly is additive: a tagged closed way that is
//! also a relation member yields its own polygon *and* contributes to the relation.
//! A closed `highway=residential` loop is a line only, never a filled blob.
//!
//! Coordinates use `decimicro / 1e7`, never `* 1e-7`, so the f64 lon/lat match
//! osmium's exactly and everything downstream lines up.
//!
//! # Merging several sources ([`Keyed`], [`par_sources`])
//!
//! Given more than one `.pbf`, every pass reads **every** source and the results
//! are folded together — there is no merged intermediate file, and nothing is
//! buffered to be sorted afterwards. Two rules define the merge, and both are
//! chosen rather than inherited:
//!
//! - **Duplicates: the first source on the command line wins, decided on the
//!   `(type, id)` alone.** Adjacent Geofabrik extracts genuinely share their
//!   border features, and the shared copies can differ — different tags, a
//!   different version, even a moved node — if the two files were downloaded on
//!   different days. Deciding on the id means a way whose *first* copy carries no
//!   style still shadows a later, tagged copy: the winner is a whole object, never
//!   a mix of two. `osmium merge` is explicitly **undefined** here (its own manual
//!   says so, and it is observably inconsistent between object types), and worse,
//!   it keeps *both* copies when their versions differ — which made the old path
//!   emit the same road twice. So there is no behaviour to match, only one to pick.
//! - **Order: ascending id, per type** — what a merged, sorted file would have
//!   handed to the same pass. This is not cosmetic. Feature order decides which
//!   quadtree chunk a feature lands in and therefore the packed bytes, so the
//!   sources' outputs are tagged with the id that produced them ([`Keyed`]) and
//!   put back in that order after the fold. A single source is already in file
//!   order and is left strictly alone — untagged, unsorted, byte-for-byte as
//!   before.
//!
//! Sources are read in parallel ([`par_sources`]); the fold that combines them is
//! sequential and in command-line order, so the result never depends on which
//! thread finished first.
//!
//! # Cropping to a `--bbox` ([`Bbox`], [`select_crop`])
//!
//! With a bbox the ingest gains a **pass 0** that reproduces `osmium extract
//! --bbox` in-process, so a cropped build needs no second C++ tool on `PATH`.
//! The strategy emulated is osmium's default, **`complete_ways`**, and matching
//! that one on purpose matters:
//!
//! - **`simple`** (keep the nodes inside the box, keep the ways touching it, and
//!   resolve nothing outside) is the naive filter, and it is actively wrong here.
//!   A way crossing the boundary would be missing node locations, and
//!   [`resolve_coords`] drops such a way *whole* — it does not trim it at the
//!   border. Every road leaving the box would disappear back to its last node
//!   inside, taking its nav-graph edges with it: the map would fray inwards and
//!   the router would lose real exits, not just geometry.
//! - **`complete_ways`** pulls in the nodes a kept way needs even when they lie
//!   outside the box. Ways stay whole, so the nav graph keeps whole edges too —
//!   an edge ends where the *way* ends, never at an arbitrary vertex on the box
//!   edge, so no phantom junction or dead-end is invented at the boundary.
//! - **`smart`** additionally completes relation members. We deliberately do not
//!   go there: it would pull in geometry osmium's default leaves out, and the
//!   committed fixtures (`apps/obc-sim/assets/repack.sh`) were packed from
//!   that default.
//!
//! Relations need no filter of their own. osmium keeps a relation iff it
//! references a kept node or way, but assembly below already requires *all*
//! member ways to be present — so a relation osmium would have dropped is one
//! whose members are all absent, and it is dropped here by that same rule.
//! Collecting every relation in pass 1 is therefore equivalent, and cheaper than
//! tracking membership.
//!
//! The cost is one extra whole-file read that collects only ids. What it buys is
//! the property that makes osmium's extract two-pass in the first place: both the
//! id sets and the pass-1 coordinate store are bounded by the *box*, not by the
//! source file, so cropping a country-sized `.pbf` stays affordable.

use std::collections::{HashMap, HashSet};

use osmpbf::{BlobReader, BlobType, ByteOffset, Element, RelMemberType};
use rayon::prelude::*;

use crate::config::Config;
use crate::geom::{assemble_multipolygon, polygon_is_valid, Geom};
use crate::hours;
use crate::nav::{self, NavGraph, RoutableWay};
use crate::poi::{self, Poi};
use crate::progress::{Phase, Progress};

pub struct IngestFeature {
    pub style_id: u8,
    pub min_lod: usize,
    pub geom: Geom,
}

/// Coastlines are captured separately (always) — they feed the bbox and land/sea.
/// POIs are the classified + deduped point-of-interest set ([`crate::poi`]),
/// serialized into the OBCM POI section (§7). `nav_graph` is the in-memory
/// routable graph ([`crate::nav`]), serialized into the v8 nav-graph section (§8).
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
    pub pois: Vec<Poi>,
    pub nav_graph: NavGraph,
}

/// A pass-1 area relation awaiting member geometry (pass 2) and assembly.
struct PendingRelation {
    style_id: u8,
    min_lod: usize,
    /// Member **way** ids in member order. Roles are dropped — `build_area`
    /// classifies outer/inner by geometry.
    member_ways: Vec<i64>,
}

/// The tags whose presence (with `area != no`) classifies a *closed* way as a
/// polygon.
const AREA_TAGS: [&str; 6] = ["building", "landuse", "amenity", "leisure", "natural", "waterway"];

/// `decimicro / 1e7`, never `* 1e-7`, so coords match osmium exactly.
#[inline]
fn to_deg(decimicro: i32) -> f64 {
    decimicro as f64 / 1e7
}

/// A `--bbox` crop region, held in the PBF's own **decimicro-degree** (`1e-7`)
/// integer grid — the same fixed point `osmium::Location` stores. Keeping the
/// edges on that grid makes [`Bbox::contains`] an integer comparison, so the
/// in-process crop cannot disagree with `osmium extract` over a node sitting a
/// float ULP from the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bbox {
    min_lon: i32,
    min_lat: i32,
    max_lon: i32,
    max_lat: i32,
}

/// Degrees → osmium's fixed point: `std::round` half-away-from-zero, same as
/// libosmium's `double_to_fix`. Rust's `f64::round` rounds the same way.
#[inline]
fn to_fix(deg: f64) -> i32 {
    (deg * 1e7).round() as i32
}

impl Bbox {
    /// Parse a `W,S,E,N` degrees spec, as strictly as `osmium extract` parses its
    /// own `--bbox`: four finite in-range numbers, west **strictly** west of east
    /// and south strictly south of north.
    ///
    /// A box wrapping the antimeridian is rejected rather than quietly packed
    /// inside-out. Every stage downstream — the header bbox, the quadtree's
    /// root box, the land clip — assumes `min < max` in plain degrees, so
    /// accepting a wrapping box would be a contract we cannot honor; osmium
    /// refuses it too. Riders who want both sides of 180° pass two boxes.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(format!("--bbox wants four comma-separated numbers W,S,E,N (got {spec:?})"));
        }
        let mut v = [0.0f64; 4];
        for (slot, text) in v.iter_mut().zip(&parts) {
            *slot = text
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| format!("--bbox: {text:?} is not a finite number (expected degrees, W,S,E,N)"))?;
        }
        let [w, s, e, n] = v;
        for (name, deg, limit) in [("west", w, 180.0), ("east", e, 180.0), ("south", s, 90.0), ("north", n, 90.0)] {
            if deg < -limit || deg > limit {
                return Err(format!("--bbox: {name} {deg} is outside ±{limit}°"));
            }
        }
        if w >= e {
            return Err(format!(
                "--bbox: west ({w}) must be strictly west of east ({e}); a box crossing the antimeridian is not \
                 supported — pack the two halves separately"
            ));
        }
        if s >= n {
            return Err(format!("--bbox: south ({s}) must be strictly south of north ({n})"));
        }
        Ok(Bbox { min_lon: to_fix(w), min_lat: to_fix(s), max_lon: to_fix(e), max_lat: to_fix(n) })
    }

    /// The box back in degrees, snapped to the decimicro grid it was parsed onto.
    /// Handed to `osmium extract` on the multi-input merge path so both croppers
    /// see the identical box.
    pub fn to_degrees(self) -> (f64, f64, f64, f64) {
        (to_deg(self.min_lon), to_deg(self.min_lat), to_deg(self.max_lon), to_deg(self.max_lat))
    }

    /// Closed on all four edges, exactly like `osmium::Box::contains`.
    #[inline]
    fn contains(&self, lon: i32, lat: i32) -> bool {
        lon >= self.min_lon && lon <= self.max_lon && lat >= self.min_lat && lat <= self.max_lat
    }
}

/// What a blob scan should do after the element it was just handed.
enum Scan {
    /// Keep going.
    Continue,
    /// Stop here. [`scan_blobs`] returns the offset of the blob this element came
    /// from, so a later pass can resume at exactly this point.
    StopAtThisBlob,
}

/// Stream a `.pbf`'s data blobs — from `start`, or from the beginning — handing
/// every element to `f`, and return the offset of the blob the scan stopped in
/// (`None` if it ran to the end of the file).
///
/// This is `ElementReader::for_each` plus the two things the passes want from the
/// file's *structure* rather than its contents: the ability to stop, and the
/// ability to resume. A sorted PBF stores nodes, then ways, then relations, and
/// the node section is ~85 % of the bytes — so a pass that only wants ways skips
/// straight to them instead of inflating and discarding every node blob. Blob
/// offsets come from the seekable reader; `start` is always an offset this
/// function returned earlier for the same file.
///
/// The blob boundary is also the ingest's cancellation checkpoint: it is the
/// coarsest unit that is still small (a few thousand elements, low single-digit
/// milliseconds), and every reading pass goes through here, so one check covers
/// passes 0, 1 and 2 on every source at once.
fn scan_blobs<F>(
    path: &str,
    start: Option<ByteOffset>,
    progress: &Progress,
    mut f: F,
) -> Result<Option<ByteOffset>, String>
where
    F: FnMut(Element) -> Scan,
{
    let mut reader = BlobReader::seekable_from_path(path).map_err(|e| format!("open {path}: {e}"))?;
    if let Some(pos) = start {
        reader.seek(pos).map_err(|e| format!("seek {path}: {e}"))?;
    }
    for blob in reader {
        progress.check()?;
        let blob = blob.map_err(|e| format!("read {path}: {e}"))?;
        // The header blob carries no elements; only OSMData blocks do.
        if !matches!(blob.get_type(), BlobType::OsmData) {
            continue;
        }
        let offset = blob.offset();
        let block = blob.to_primitiveblock().map_err(|e| format!("decode {path}: {e}"))?;
        for el in block.elements() {
            if let Scan::StopAtThisBlob = f(el) {
                return Ok(offset);
            }
        }
    }
    Ok(None)
}

/// Run `f` over every source in parallel, collecting the results **in source
/// order**.
///
/// The sources are the natural unit of parallelism here: each pass reads each
/// file independently, and only the fold that combines them has to be ordered.
/// It is also where the wall clock the old `osmium merge` path got from
/// multi-core blob decoding comes back — one thread per input rather than one
/// thread per blob.
fn par_sources<T, F>(paths: &[String], f: F) -> Result<Vec<T>, String>
where
    T: Send,
    F: Fn(usize, &str) -> Result<T, String> + Sync,
{
    paths.par_iter().enumerate().map(|(i, p)| f(i, p.as_str())).collect()
}

/// Per-element output tagged with the id of the OSM object that produced it.
///
/// The tag is what lets several `.pbf`s be read independently and still come out
/// exactly as one merged file would have produced them: later copies of an
/// already-seen object dropped ([`Keyed::retain_keys`]), everything back in id
/// order ([`Keyed::sort`]). With a single source there is nothing to merge, so
/// nothing is tagged and this is a plain `Vec<T>` — an uncropped country pack
/// must not start paying 8 bytes per feature for a merge it isn't doing.
struct Keyed<T> {
    tagged: bool,
    keys: Vec<i64>,
    items: Vec<T>,
}

impl<T> Keyed<T> {
    fn new(tagged: bool) -> Self {
        Keyed { tagged, keys: Vec::new(), items: Vec::new() }
    }

    #[inline]
    fn push(&mut self, key: i64, item: T) {
        if self.tagged {
            self.keys.push(key);
        }
        self.items.push(item);
    }

    /// Concatenate a later source's outputs onto this one.
    fn append(&mut self, mut other: Self) {
        self.keys.append(&mut other.keys);
        self.items.append(&mut other.items);
    }

    /// Drop every item whose key `keep` rejects, preserving order. Tagged only —
    /// it is a merge operation and never runs on a single-source ingest.
    fn retain_keys(&mut self, mut keep: impl FnMut(i64) -> bool) {
        debug_assert!(self.tagged && self.keys.len() == self.items.len());
        let mut w = 0;
        for r in 0..self.items.len() {
            if keep(self.keys[r]) {
                if w != r {
                    self.keys.swap(w, r);
                    self.items.swap(w, r);
                }
                w += 1;
            }
        }
        self.keys.truncate(w);
        self.items.truncate(w);
    }

    /// Put the items back in ascending-id order — the order a merged, sorted
    /// `.pbf` would have handed them to the same pass.
    ///
    /// The sort is **stable**, so a file that repeats an id inside itself (a
    /// history file, which the fold's cross-source dedup never sees) keeps its own
    /// order instead of picking one arbitrarily. Determinism is the whole point of
    /// this function; it must not have a case where it flips a coin.
    ///
    /// The already-sorted check is not just an optimization: sources are normally
    /// sorted and mostly disjoint, so the common case is a couple of runs that are
    /// already in order, and skipping keeps the transient pair vector — the only
    /// copy of the payload this whole merge makes — out of the picture entirely.
    fn sort(&mut self) {
        debug_assert!(self.tagged && self.keys.len() == self.items.len());
        if self.keys.is_sorted() {
            return;
        }
        let keys = std::mem::take(&mut self.keys);
        let items = std::mem::take(&mut self.items);
        let mut pairs: Vec<(i64, T)> = keys.into_iter().zip(items).collect();
        pairs.sort_by_key(|(k, _)| *k);
        (self.keys, self.items) = pairs.into_iter().unzip();
    }

    fn into_items(self) -> Vec<T> {
        self.items
    }
}

/// A grow-then-freeze set of OSM ids, backed by a sorted `Vec`.
///
/// The crop's three id sets are the memory floor of a `--bbox` run over a large
/// source, so this trades a `HashSet`'s per-entry overhead for 8 flat bytes and a
/// binary search. It works because each set is filled in one pass and only read
/// in a later one; [`IdSet::freeze`] runs at that seam. `contains` on an unfrozen
/// set would silently lie, so freezing is the type's one rule.
#[derive(Default)]
struct IdSet(Vec<i64>);

impl IdSet {
    /// Take another source's ids wholesale. Moves the first batch instead of
    /// copying it, so the single-source case allocates once.
    fn absorb(&mut self, mut ids: Vec<i64>) {
        if self.0.is_empty() {
            self.0 = ids;
        } else {
            self.0.append(&mut ids);
        }
    }

    /// End the fill phase. Idempotent, so pass 0 can freeze the node set early
    /// (the first way needs it) and freeze the rest at the end without tracking
    /// which already happened.
    fn freeze(&mut self) {
        self.0.sort_unstable();
        self.0.dedup();
        self.0.shrink_to_fit();
    }

    #[inline]
    fn contains(&self, id: i64) -> bool {
        self.0.binary_search(&id).is_ok()
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// The id sets that define a `--bbox` crop — `osmium extract`'s `complete_ways`
/// selection, computed in-process (see the module docs).
pub struct Crop {
    /// Nodes whose location falls inside the box.
    inside: IdSet,
    /// Nodes *outside* the box that a kept way still references — the halo that
    /// keeps boundary-crossing ways whole.
    halo: IdSet,
    /// Ways with at least one node inside the box.
    ways: IdSet,
}

impl Crop {
    /// Nodes the extract would contain: inside the box, or needed by a kept way.
    #[inline]
    fn keeps_node(&self, id: i64) -> bool {
        self.inside.contains(id) || self.halo.contains(id)
    }

    #[inline]
    fn keeps_way(&self, id: i64) -> bool {
        self.ways.contains(id)
    }

    /// Nothing at all inside the box *and* no way reaching into it — the caller
    /// should fail loudly rather than pack an empty map.
    fn is_empty(&self) -> bool {
        self.inside.len() == 0 && self.ways.len() == 0
    }
}

/// **Pass 0** — select the crop across every source: nodes inside `bbox`, ways
/// touching one of them, and the outside nodes those ways still need. Also
/// returns, per source, the offset of the first blob that holds a way — pass 2
/// resumes there instead of decoding the node section a third time.
///
/// Two phases rather than one sweep, and that is a merge requirement rather than
/// a refactor: a way in one file can have its only in-box node in *another* file
/// (adjacent extracts share their border, and one side may hold the node while
/// the other holds the way), so the node phase has to finish across **all**
/// sources before any file's ways can be judged. The split costs nothing,
/// because it falls where the file is already split: phase A walks the node
/// section and stops at the first way-bearing blob, phase B resumes exactly
/// there. Together they decode the file once, the same as the single sweep they
/// replace.
///
/// Passes 1 and 2 don't care about element order (they are separate reads, and
/// relations only carry ids), so this is the one place that does: phase A stops
/// at the first way, so a node *after* a way would be silently skipped. Phase B
/// sees the whole tail and turns that into an error rather than a quietly wrong
/// crop.
fn select_crop(paths: &[String], bbox: Bbox, progress: &Progress) -> Result<(Crop, Vec<Option<ByteOffset>>), String> {
    progress.stage(Phase::Ingest, "Pass 0: selecting bbox...");
    // --- Phase A: the in-box node ids, from every source. ---
    let scans = par_sources(paths, |_, path| {
        let mut ids: Vec<i64> = Vec::new();
        let mut in_box = |lon: i32, lat: i32, id: i64| {
            if bbox.contains(lon, lat) {
                ids.push(id);
            }
        };
        let ways_at = scan_blobs(path, None, progress, |el| match el {
            Element::Node(n) => {
                in_box(n.decimicro_lon(), n.decimicro_lat(), n.id());
                Scan::Continue
            }
            Element::DenseNode(n) => {
                in_box(n.decimicro_lon(), n.decimicro_lat(), n.id());
                Scan::Continue
            }
            // The first way: the node section is behind us and phase B takes over
            // from this blob. (A file with no ways at all just runs to the end.)
            Element::Way(_) => Scan::StopAtThisBlob,
            Element::Relation(_) => Scan::Continue,
        })?;
        Ok((ids, ways_at))
    })?;
    let mut inside = IdSet::default();
    let mut ways_at = Vec::with_capacity(paths.len());
    for (ids, at) in scans {
        inside.absorb(ids);
        ways_at.push(at);
    }
    inside.freeze();

    // --- Phase B: the ways that touch the box, and the halo they still need. ---
    let scans = par_sources(paths, |i, path| {
        let (mut ways, mut halo) = (Vec::new(), Vec::new());
        let (mut saw_way, mut out_of_order) = (false, false);
        scan_blobs(path, ways_at[i], progress, |el| {
            match el {
                Element::Way(w) => {
                    saw_way = true;
                    if w.refs().any(|r| inside.contains(r)) {
                        ways.push(w.id());
                        // The halo: every other node this way needs. Ids already
                        // `inside` are skipped — `keeps_node` checks both sets, and a
                        // dense urban box would otherwise store most of its nodes twice.
                        for r in w.refs() {
                            if !inside.contains(r) {
                                halo.push(r);
                            }
                        }
                    }
                }
                // Nodes before the first way of the resume blob are ones phase A
                // already saw; anything after a way means the file isn't sorted.
                Element::Node(_) | Element::DenseNode(_) => out_of_order |= saw_way,
                Element::Relation(_) => {}
            }
            Scan::Continue
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a node follows a way), so --bbox cannot select its ways — sort it first \
                 (e.g. `osmium sort`)"
            ));
        }
        Ok((ways, halo))
    })?;
    let (mut ways, mut halo) = (IdSet::default(), IdSet::default());
    for (w, h) in scans {
        ways.absorb(w);
        halo.absorb(h);
    }
    ways.freeze();
    halo.freeze();
    progress.log(format!(
        "  {} node(s) in box, {} way(s) kept (+{} boundary node(s))",
        inside.len(),
        ways.len(),
        halo.len()
    ));
    Ok((Crop { inside, halo, ways }, ways_at))
}

/// One source's pass-1 harvest.
struct NodeScan {
    nodes: HashMap<i64, (i32, i32)>,
    pois: Keyed<Poi>,
    rels: Keyed<PendingRelation>,
}

/// One source's pass-2 harvest, plus the way ids it claims.
struct WayScan {
    features: Keyed<IngestFeature>,
    coastlines: Keyed<Vec<(f64, f64)>>,
    pois: Keyed<Poi>,
    routable: Keyed<RoutableWay>,
    member_geom: HashMap<i64, Vec<(f64, f64)>>,
    /// Every way id this source processed — **including** the ones that produced
    /// nothing at all. Ownership is decided on the id alone, so an untagged or
    /// unresolvable copy here still has to shadow a later source's copy; a list
    /// of what actually came *out* could not say that. Left empty for a single
    /// source, which claims everything by definition.
    claimed: Vec<i64>,
}

/// What an ingest does with the routable ways it collected.
enum NavMode {
    /// Build the whole-extract nav graph and drop the ways — the ordinary pack.
    Graph,
    /// Keep the ways and build no graph: the cell cutter builds **one graph per cell** from them
    /// (OBCA §3.4), so a whole-extract graph would be wasted work whose island pruning is also the
    /// wrong shape for a cell.
    KeepWays,
}

/// Two-pass ingest of one or more `.osm.pbf`s (lines + closed-way polygons +
/// relation-assembled area polygons), merged as described in the module docs.
/// `bbox` crops the inputs to a box first (a third, id-only pass).
pub fn ingest_osm(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<Ingested, String> {
    ingest_inner(paths, config, bbox, progress, NavMode::Graph).map(|(ing, _)| ing)
}

/// [`ingest_osm`], but returning the **routable ways** instead of a built nav graph — what
/// [`crate::cut`] needs, because a cell classifies junctions from the source snapshot's whole way set
/// and cuts the ways itself at the cell edges. The returned [`Ingested`] carries an empty
/// `nav_graph`.
pub fn ingest_osm_ways(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<(Ingested, Vec<RoutableWay>), String> {
    ingest_inner(paths, config, bbox, progress, NavMode::KeepWays)
}

fn ingest_inner(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
    nav_mode: NavMode,
) -> Result<(Ingested, Vec<RoutableWay>), String> {
    if paths.is_empty() {
        return Err("no .osm.pbf input given".into());
    }
    // More than one source ⇒ every output gets tagged with the id that produced
    // it, which is what the fold needs to drop duplicates and restore the order a
    // single merged file would have had.
    let merging = paths.len() > 1;
    if merging {
        progress.stage(
            Phase::Merging,
            format!("Merging {} sources (on a duplicate id, the first source wins)...", paths.len()),
        );
    }

    // --- Pass 0 (only with --bbox): the `complete_ways` id selection, plus the
    // per-source offset where the ways begin (pass 2 resumes there). ---
    let (crop, ways_at) = match bbox {
        Some(bb) => {
            let (crop, ways_at) = select_crop(paths, bb, progress)?;
            if crop.is_empty() {
                let (w, s, e, n) = bb.to_degrees();
                return Err(format!("--bbox {w},{s},{e},{n} does not overlap any data in {}", paths.join(", ")));
            }
            (Some(crop), ways_at)
        }
        None => (None, vec![None; paths.len()]),
    };

    // --- Pass 1: node-location store + relation collection, per source. ---
    // The stage strings reach the build UI (scraped from stdout by the dev server,
    // delivered as events by the desktop app) — report each when its pass actually
    // starts, not both up front.
    progress.stage(Phase::Ingest, "Pass 1: reading nodes...");
    let scans = par_sources(paths, |_, path| read_nodes(path, config, crop.as_ref(), merging, progress))?;
    let NodeScan { nodes, pois: node_pois, rels } = fold_node_scans(scans, merging);
    let pending = rels.into_items();
    let needed_ways: HashSet<i64> = pending.iter().flat_map(|r| r.member_ways.iter().copied()).collect();

    // --- Pass 2: ways → features + coastlines, plus member-way geometry capture. ---
    progress.stage(Phase::Ingest, "Pass 2: processing ways...");
    let scans = par_sources(paths, |i, path| {
        read_ways(path, ways_at[i], config, crop.as_ref(), &nodes, &needed_ways, merging, progress)
    })?;
    let WayScan { features, coastlines, pois: way_pois, routable, member_geom, .. } = fold_way_scans(scans, merging);
    let mut features = features.into_items();
    let coastlines = coastlines.into_items();
    let routable_ways = routable.into_items();
    // POI candidates from both passes, deduped after assembly — node candidates
    // first, then way centroids, the order a single sorted file produces them in.
    // Classification is config-free (hardcoded table — locked decision on #115).
    let mut poi_cands = node_pois.into_items();
    poi_cands.extend(way_pois.into_items());

    // --- Assemble relation areas from captured member geometry. ---
    // Each outer ring (+ nested holes) becomes one polygon, styled by the relation.
    // **Completeness:** like osmium, only assemble when ALL member ways are present;
    // an incomplete relation (a member clipped out of the extract) is dropped, not
    // assembled from survivors — that would emit a phantom boundary-crossing polygon.
    for pr in &pending {
        let mut members = Vec::with_capacity(pr.member_ways.len());
        let mut complete = true;
        for wid in &pr.member_ways {
            match member_geom.get(wid) {
                Some(g) => members.push(g.clone()),
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            continue;
        }
        for poly in assemble_multipolygon(&members) {
            features.push(IngestFeature { style_id: pr.style_id, min_lod: pr.min_lod, geom: poly });
        }
    }

    // --- POIs: collapse OSM double-mapping, then log per-category counts. ---
    let (pois, poi_dropped) = poi::dedupe(poi_cands);
    progress.log(poi::format_counts(&pois, poi_dropped));

    // --- Nav graph: junctions + deduped edges from the routable ways, then
    // island pruning (`routing.min_component_edges`) + v9-guarantee edge splits
    // ([`nav::build_graph_with`]). Serialized into the §8 nav section. Logged (with
    // component + kinds stats) alongside POIs.
    let (nav_graph, kept_ways) = match nav_mode {
        NavMode::Graph => {
            let (graph, stats) = nav::build_graph_with(&routable_ways, config.routing.min_component_edges);
            progress.log(nav::format_summary(&graph, &stats));
            (graph, Vec::new())
        }
        NavMode::KeepWays => {
            progress.log(format!("routable ways: {} (graphs are built per cell)", routable_ways.len()));
            (NavGraph::default(), routable_ways)
        }
    };

    Ok((Ingested { features, coastlines, pois, nav_graph }, kept_ways))
}

/// **Pass 1**, one source: node-location store + node POIs + area relations.
///
/// Cropped, this keeps only the nodes the extract would contain — which includes
/// the halo, so a tagged node just outside the box that a kept way needs becomes
/// a POI here exactly as it would in an `osmium extract` output (osmium writes
/// those nodes whole, tags and all). Matching that is the point.
///
/// Relations are collected unfiltered even when cropping: the all-members-present
/// rule in [`ingest_osm`] already drops exactly the ones osmium's crop would have
/// left out (module docs).
fn read_nodes(
    path: &str,
    config: &Config,
    crop: Option<&Crop>,
    tagged: bool,
    progress: &Progress,
) -> Result<NodeScan, String> {
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut pois = Keyed::new(tagged);
    let mut rels = Keyed::new(tagged);
    let keeps_node = |id: i64| crop.is_none_or(|c| c.keeps_node(id));
    scan_blobs(path, None, progress, |el| {
        match el {
            Element::Node(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut pois);
            }
            Element::DenseNode(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut pois);
            }
            Element::Relation(r) => collect_relation(&r, config, &mut rels),
            _ => {}
        }
        Scan::Continue
    })
    .map_err(|e| format!("pass 1: {e}"))?;
    Ok(NodeScan { nodes, pois, rels })
}

/// Combine the sources' pass-1 harvests, in command-line order.
fn fold_node_scans(scans: Vec<NodeScan>, merging: bool) -> NodeScan {
    let mut it = scans.into_iter();
    let mut acc = it.next().expect("at least one source");
    let mut seen_rels: HashSet<i64> = acc.rels.keys.iter().copied().collect();
    for mut next in it {
        // Ownership is tested BEFORE this source's nodes land in `acc`, so the
        // question is "did an earlier source already have this node?" — and the
        // whole object loses, tags and all, not just its coordinate.
        next.pois.retain_keys(|id| !acc.nodes.contains_key(&id));
        next.rels.retain_keys(|id| seen_rels.insert(id));
        acc.pois.append(next.pois);
        acc.rels.append(next.rels);
        for (id, coord) in next.nodes {
            acc.nodes.entry(id).or_insert(coord);
        }
    }
    if merging {
        acc.pois.sort();
        acc.rels.sort();
    }
    acc
}

/// **Pass 2**, one source: ways → features + coastlines + POIs + routable
/// topology, and the geometry of any way a relation needs.
///
/// `ways_at` is where pass 0 found this file's first way; starting there skips
/// re-decoding the node section, which is the bulk of a `.pbf`'s bytes. Without a
/// `--bbox` there is no pass 0 and hence no offset, and the scan starts at the
/// beginning — which is also what keeps an uncropped ingest order-agnostic.
#[allow(clippy::too_many_arguments)]
fn read_ways(
    path: &str,
    ways_at: Option<ByteOffset>,
    config: &Config,
    crop: Option<&Crop>,
    nodes: &HashMap<i64, (i32, i32)>,
    needed_ways: &HashSet<i64>,
    tagged: bool,
    progress: &Progress,
) -> Result<WayScan, String> {
    let mut features = Keyed::new(tagged);
    let mut coastlines = Keyed::new(tagged);
    let mut pois = Keyed::new(tagged);
    // Routable-way topology for the nav graph ([`crate::nav`]). We keep the OSM node
    // ids here (which the render path drops) so shared nodes can be recovered as
    // junctions after the pass; the graph is built from these once all ways are seen.
    let mut routable = Keyed::new(tagged);
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    let mut claimed: Vec<i64> = Vec::new();
    let keeps_way = |id: i64| crop.is_none_or(|c| c.keeps_way(id));
    scan_blobs(path, ways_at, progress, |el| {
        if let Element::Way(w) = el {
            if keeps_way(w.id()) {
                // Claimed on sight, before anything can go wrong with it: a way
                // this source could not resolve still shadows a later copy.
                if tagged {
                    claimed.push(w.id());
                }
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way — osmium would raise
                // `InvalidLocationError` here, and the way is dropped.
                if let Some(coords) = resolve_coords(&refs, nodes) {
                    push_routable_way(w.id(), &w, &refs, &coords, &mut routable);
                    process_way(&w, &refs, &coords, config, &mut features, &mut coastlines, &mut pois);
                    if needed_ways.contains(&w.id()) {
                        member_geom.insert(w.id(), coords);
                    }
                }
            }
        }
        Scan::Continue
    })
    .map_err(|e| format!("pass 2: {e}"))?;
    Ok(WayScan { features, coastlines, pois, routable, member_geom, claimed })
}

/// Combine the sources' pass-2 harvests, in command-line order: a later source
/// contributes only the ways no earlier source claimed, and the survivors are put
/// back in way-id order.
fn fold_way_scans(scans: Vec<WayScan>, merging: bool) -> WayScan {
    let mut it = scans.into_iter();
    let mut acc = it.next().expect("at least one source");
    let mut claimed = IdSet::default();
    claimed.absorb(std::mem::take(&mut acc.claimed));
    claimed.freeze();
    for mut next in it {
        let owned = |id: i64| !claimed.contains(id);
        next.features.retain_keys(owned);
        next.coastlines.retain_keys(owned);
        next.pois.retain_keys(owned);
        next.routable.retain_keys(owned);
        acc.features.append(next.features);
        acc.coastlines.append(next.coastlines);
        acc.pois.append(next.pois);
        acc.routable.append(next.routable);
        for (id, geom) in next.member_geom {
            if owned(id) {
                acc.member_geom.insert(id, geom);
            }
        }
        claimed.absorb(next.claimed);
        claimed.freeze();
    }
    if merging {
        acc.features.sort();
        acc.coastlines.sort();
        acc.pois.sort();
        acc.routable.sort();
    }
    acc
}

/// Capture a routable way's node-id sequence + µdeg coords for the nav graph.
/// Routability is tag-based ([`nav::is_routable`]) and independent of styling — a
/// way can be routable without a render style and vice-versa. Ways with fewer than
/// two nodes carry no edge and are skipped. `coords` is the way's f64-degree
/// geometry from [`resolve_coords`]; it is snapped to the µdeg grid here (the same
/// grid POIs and the serializer use) so edge lengths and later serialization agree.
fn push_routable_way(id: i64, w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], out: &mut Keyed<RoutableWay>) {
    if refs.len() < 2 {
        return;
    }
    // Classify once (routability + way-kind byte). `None` ⇒ not routable — this is
    // the only place tags exist, so the kind is captured here or never.
    let Some(kind) = nav::classify(w.tags()) else { return };
    let coords_udeg = coords.iter().map(|&(x, y)| (poi::to_udeg(x), poi::to_udeg(y))).collect();
    out.push(id, RoutableWay { node_ids: refs.to_vec(), coords: coords_udeg, kind });
}

/// Classify one node's tags against the POI table; push a candidate on match.
/// The overwhelmingly common untagged-node case falls straight through.
fn push_node_poi<'a, I>(id: i64, tags: I, decimicro_lon: i32, decimicro_lat: i32, out: &mut Keyed<Poi>)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    if let Some((subtype, name, raw_hours)) = poi::classify(tags) {
        out.push(
            id,
            Poi {
                subtype,
                lon_udeg: poi::to_udeg(to_deg(decimicro_lon)),
                lat_udeg: poi::to_udeg(to_deg(decimicro_lat)),
                name,
                from_node: true,
                hours: raw_hours.and_then(hours::parse),
            },
        );
    }
}

/// Resolve a way's node refs to degree coordinates. `None` iff any node is missing
/// — the caller drops the way (osmium's `InvalidLocationError`).
fn resolve_coords(refs: &[i64], nodes: &HashMap<i64, (i32, i32)>) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for r in refs {
        let &(dx, dy) = nodes.get(r)?;
        coords.push((to_deg(dx), to_deg(dy)));
    }
    Some(coords)
}

/// Collect a `type=multipolygon`/`type=boundary` relation (skipping `admin_level`)
/// for area assembly: record its style + member way-ids. Roles are ignored;
/// non-way members are skipped.
fn collect_relation(r: &osmpbf::Relation, config: &Config, pending: &mut Keyed<PendingRelation>) {
    let tags: HashMap<&str, &str> = r.tags().collect();
    match tags.get("type").copied() {
        Some("multipolygon") | Some("boundary") => {}
        _ => return,
    }
    // admin_level relations are line-only → no polygon.
    if tags.contains_key("admin_level") {
        return;
    }
    let Some(style) = config.get_style(&tags) else { return };
    let member_ways: Vec<i64> =
        r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
    if member_ways.is_empty() {
        return;
    }
    pending.push(r.id(), PendingRelation { style_id: style.id, min_lod: style.min_lod, member_ways });
}

/// One way: capture coastline always, then style + classify into a single
/// polygon-or-line emission. `refs`/`coords` are pre-resolved.
fn process_way(
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    config: &Config,
    features: &mut Keyed<IngestFeature>,
    coastlines: &mut Keyed<Vec<(f64, f64)>>,
    pois: &mut Keyed<Poi>,
) {
    let tags: HashMap<&str, &str> = w.tags().collect();
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled — and
    // as lines, never areas.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(w.id(), coords.to_vec());
    }

    // A closed way matching the POI table yields a POI at the ring centroid —
    // independent of styling (a bare `shop=supermarket` outline has no style at
    // all). The building-tagged supermarket way and the area campsite are the
    // motivating cases; relations are out of scope (#115).
    if is_closed {
        if let Some((subtype, name, raw_hours)) = poi::classify(tags.iter().map(|(&k, &v)| (k, v))) {
            let (cx, cy) = poi::ring_centroid(coords);
            pois.push(
                w.id(),
                Poi {
                    subtype,
                    lon_udeg: poi::to_udeg(cx),
                    lat_udeg: poi::to_udeg(cy),
                    name,
                    from_node: false,
                    hours: raw_hours.and_then(hours::parse),
                },
            );
        }
    }

    let Some(style) = config.get_style(&tags) else { return };

    // A closed area emits a polygon; a closed road loop emits a line, never both.
    if is_closed && is_area(&tags) {
        // admin_level + area ⇒ drop entirely (no line, no polygon).
        if tags.contains_key("admin_level") {
            return;
        }
        // Skip rings osmium's assembler would reject as invalid (e.g. a
        // self-intersecting building); no polygon and no line (line branch returned).
        if coords.len() >= 3 && polygon_is_valid(coords, &[]) {
            features.push(
                w.id(),
                IngestFeature {
                    style_id: style.id,
                    min_lod: style.min_lod,
                    geom: Geom::Polygon { exterior: coords.to_vec(), interiors: Vec::new() },
                },
            );
        }
        return;
    }

    // Line: open ways, and closed-but-not-area circular roads.
    if coords.len() >= 2 {
        features.push(
            w.id(),
            IngestFeature { style_id: style.id, min_lod: style.min_lod, geom: Geom::Line(coords.to_vec()) },
        );
    }
}

/// Closed-way area heuristic: `area=yes` ⇒ area; `area=no` ⇒ never; otherwise
/// area iff it carries any [`AREA_TAGS`] key.
fn is_area(tags: &HashMap<&str, &str>) -> bool {
    match tags.get("area") {
        Some(&"yes") => true,
        Some(&"no") => false,
        _ => AREA_TAGS.iter().any(|k| tags.contains_key(k)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny.osm.pbf");

    fn is_polygon(g: &Geom) -> bool {
        matches!(g, Geom::Polygon { .. })
    }

    /// `ingest_osm` takes the whole source list; most tests hand it exactly one.
    fn sources(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    /// No test here watches the narration or cancels a run; cancellation has its
    /// own tests in [`crate::pipeline`], where a whole pack can be stopped.
    fn quiet() -> Progress {
        Progress::silent()
    }

    /// Everything an ingest produced, flattened into one comparable value —
    /// enough to say "these two runs are the same map", including *order*, which
    /// decides the packed bytes downstream.
    fn shape(ing: &Ingested) -> Vec<String> {
        let mut out: Vec<String> =
            ing.features.iter().map(|f| format!("F {} {} {:?}", f.style_id, f.min_lod, f.geom.bounds())).collect();
        out.extend(ing.coastlines.iter().map(|c| format!("C {c:?}")));
        out.extend(ing.pois.iter().map(|p| format!("P {} {} {} {:?}", p.subtype, p.lon_udeg, p.lat_udeg, p.name)));
        out.push(format!("nav {} nodes, {} edges", ing.nav_graph.nodes.len(), ing.nav_graph.edges.len()));
        out
    }

    /// The `tiny.osm` truth table: relations assembled (R1's lake with a hole, R2's
    /// two forest outers) plus lines and closed-way polygons → 10 features.
    #[test]
    fn tiny_truth_table() {
        // The fixture is committed in-repo (source of truth `tiny/tiny.osm`); a
        // missing fixture is a hard failure, not a skip.
        assert!(
            std::path::Path::new(TINY_PBF).exists(),
            "corpus fixture missing: {TINY_PBF}. It is committed; rebuild from tiny/tiny.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let ing = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");

        // W8 (way 109) is the only coastline; nodes 29,30 ⇒ 2 points.
        assert_eq!(ing.coastlines.len(), 1, "exactly one coastline");
        assert_eq!(ing.coastlines[0].len(), 2);

        // Multiset of (style_id, is_polygon).
        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        // Style ids: forest=40, pedestrian=15, residential=12, primary=5,
        // trunk=3, admin_level/2=50, water=32 (see config doc order).
        assert_eq!(n(40, true), 3, "W5 closed forest + R2's two outer rings ⇒ 3 polygons");
        assert_eq!(n(32, true), 1, "R1 natural=water ⇒ 1 polygon (lake)");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(50, false), 1, "W9 admin_level=2 ⇒ 1 line");
        assert_eq!(n(32, false), 1, "W12 natural=water area=no ⇒ 1 line");

        // R1 is a lake WITH an island (one hole).
        let lake = ing.features.iter().find(|f| f.style_id == 32 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "R1 has one hole"),
            _ => unreachable!(),
        }

        // The fixes/omissions we MUST honor:
        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        // 5 polygons (3 forest, 1 pedestrian, 1 water lake) + 5 lines.
        assert_eq!(ing.features.len(), 10, "10 features total");
    }

    /// End-to-end POI extraction over the hand-authored `poi.osm` fixture (its
    /// header comment is the truth table): node + closed-way classification,
    /// name folding, and both dedup pairs (node-beats-centroid, named-beats-
    /// unnamed). See builder/tests/corpus/poi/poi.osm.
    #[test]
    fn poi_fixture_end_to_end() {
        const POI_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/poi.osm.pbf");
        assert!(
            std::path::Path::new(POI_PBF).exists(),
            "corpus fixture missing: {POI_PBF}. It is committed; rebuild from poi/poi.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let ing = ingest_osm(&sources(&[POI_PBF]), &cfg, None, &quiet()).expect("ingest");

        // 7 candidates (5 nodes + 2 way-centroids), 2 dedup-dropped ⇒ 5 kept.
        assert_eq!(ing.pois.len(), 5, "expected 5 POIs, got: {:?}", ing.pois);

        let find = |name: Option<&str>, subtype: u8| {
            ing.pois
                .iter()
                .find(|p| p.subtype == subtype && p.name.as_deref() == name)
                .unwrap_or_else(|| panic!("missing poi subtype {subtype} name {name:?}: {:?}", ing.pois))
        };

        // N1: named water node, exact µdeg grid.
        let n1 = find(Some("Marktbrunnen"), 1);
        assert_eq!((n1.lat_udeg, n1.lon_udeg, n1.from_node), (47_995_000, 7_850_000, true));
        // N2 beat W1's centroid: node position (the building corner), way's
        // name folded ü→ue at pack time.
        let n2 = find(Some("Edeka Mueller"), 13);
        assert_eq!((n2.lat_udeg, n2.lon_udeg, n2.from_node), (47_989_900, 7_859_900, true));
        // N3: CJK name folded to empty ⇒ unnamed.
        let n3 = find(None, 1);
        assert_eq!((n3.lat_udeg, n3.lon_udeg), (47_980_000, 7_840_000));
        // N5 beat the unnamed spring N6 40 m away (named > unnamed, same category).
        find(Some("Brunnen A"), 1);
        assert!(!ing.pois.iter().any(|p| p.subtype == 2), "spring N6 must be dedup-dropped");
        // W2: unnamed campsite way ⇒ POI at the ring centroid.
        let w2 = find(None, 5);
        assert_eq!((w2.lat_udeg, w2.lon_udeg, w2.from_node), (48_000_200, 7_870_200, false));
        // N4 (amenity=parking) never classified.
        assert_eq!(crate::poi::format_counts(&ing.pois, 0).matches("water 3").count(), 1);
    }

    /// The `--bbox` contract is user-facing, so the parser is as strict as
    /// `osmium extract`'s: four in-range numbers, west of east, south of north.
    #[test]
    fn bbox_parse_is_strict_about_the_box() {
        let ok = Bbox::parse("7.39,43.71,7.47,43.77").expect("valid box");
        assert_eq!(ok.to_degrees(), (7.39, 43.71, 7.47, 43.77), "degrees survive the decimicro round trip");
        assert_eq!(Bbox::parse(" 7.39 , 43.71 , 7.47 , 43.77 ").expect("whitespace"), ok, "fields are trimmed");
        // The edges land on osmium's grid: round-half-away-from-zero at 1e-7.
        assert_eq!(to_fix(7.39), 73_900_000);
        assert_eq!(to_fix(-7.39), -73_900_000);

        for bad in [
            "7.39,43.71,7.47",         // three fields
            "7.39,43.71,7.47,43.77,1", // five
            "west,43.71,7.47,43.77",   // not a number
            "nan,43.71,7.47,43.77",    // not finite
            "-181,43.71,7.47,43.77",   // lon out of range
            "7.39,-91,7.47,43.77",     // lat out of range
            "7.47,43.71,7.39,43.77",   // east of west (the antimeridian wrap)
            "7.39,43.71,7.39,43.77",   // zero width
            "7.39,43.77,7.47,43.71",   // north below south
        ] {
            assert!(Bbox::parse(bad).is_err(), "{bad:?} must be rejected");
        }
        // A wrapping box names the reason, not just "invalid".
        let msg = Bbox::parse("179,-1,-179,1").unwrap_err();
        assert!(msg.contains("antimeridian"), "wrap error should explain itself: {msg}");
    }

    /// The `complete_ways` crop, over the `tiny.osm` truth table. The box covers
    /// R1 whole, takes only one of R2's two outer rings, and clips the middle of
    /// both open highways:
    ///
    /// - **ways stay whole**: W7b (trunk) reaches to lon 7.855, far outside the
    ///   box, because one of its nodes is inside. That is the property `simple`
    ///   would lose — and losing it would delete the way outright here, since
    ///   [`resolve_coords`] drops a way with any unresolvable node.
    /// - **relations stay all-or-nothing**: R2 lost member W4, so it is dropped
    ///   entirely rather than assembled from the surviving ring.
    #[test]
    fn bbox_crop_keeps_ways_whole_and_relations_all_or_nothing() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        // lon 7.798..7.809, lat 47.979..47.995 — see tiny.osm's node grid.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let ing = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");

        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        // R1 (both member ways inside) still assembles, hole and all.
        assert_eq!(n(32, true), 1, "R1 lake survives whole");
        let lake = ing.features.iter().find(|f| f.style_id == 32 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "island hole kept"),
            _ => unreachable!(),
        }
        // R2 kept W3 but lost W4 ⇒ no forest at all, not a half-forest.
        assert_eq!(n(39, true), 0, "R2 is incomplete ⇒ dropped, never assembled from survivors");
        // Out of the box entirely: W5/W6/W11 (lat ≥ 47.996), W9 (48.000), W8 coast.
        assert_eq!(n(15, true), 0, "W11 pedestrian area is north of the box");
        assert_eq!(n(12, false), 0, "W6 residential loop is north of the box");
        assert_eq!(n(42, false), 0, "W9 admin line is north of the box");
        assert!(ing.coastlines.is_empty(), "W8 coastline sits east of the box");
        // Kept: W7 primary, W7b trunk, W12 water line — plus R1's polygon.
        assert_eq!(n(5, false), 1, "W7 primary crosses the east edge and is kept");
        assert_eq!(n(3, false), 1, "W7b trunk crosses the east edge and is kept");
        assert_eq!(n(32, false), 1, "W12 water line is inside");
        assert_eq!(ing.features.len(), 4, "1 lake polygon + 3 lines");

        // The headline: the trunk is not trimmed at the box edge (lon 7.809) — it
        // keeps its far node at 7.855, exactly as `osmium extract` would emit it.
        let trunk = ing.features.iter().find(|f| f.style_id == 3).expect("trunk line");
        let (_, _, maxx, _) = trunk.geom.bounds();
        assert!((maxx - 7.855).abs() < 1e-9, "trunk must reach its real end at 7.855, got {maxx}");
    }

    /// A box that swallows the whole file must change nothing — the crop path is
    /// a filter, not a second code path with its own behaviour.
    #[test]
    fn bbox_covering_everything_is_a_no_op() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let plain = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let boxed =
            ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(Bbox::parse("-180,-90,180,90").expect("world")), &quiet())
                .expect("ingest");
        assert_eq!(plain.features.len(), boxed.features.len());
        assert_eq!(plain.coastlines, boxed.coastlines);
        assert_eq!(plain.pois.len(), boxed.pois.len());
        for (a, b) in plain.features.iter().zip(&boxed.features) {
            assert_eq!((a.style_id, a.min_lod, a.geom.bounds()), (b.style_id, b.min_lod, b.geom.bounds()));
        }
    }

    /// A box over empty water fails with a sentence naming the box, rather than
    /// packing a valid-but-empty `.obcm` the rider only discovers on the device.
    #[test]
    fn bbox_missing_the_data_is_an_error() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let Err(err) =
            ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(Bbox::parse("10,10,11,11").expect("box")), &quiet())
        else {
            panic!("a box off in the Mediterranean must not ingest");
        };
        assert!(err.contains("does not overlap"), "unexpected message: {err}");
    }

    /// Pass 0 is the one place that needs the PBF type-sorted, and a file that
    /// isn't would otherwise select nothing at all and pack a silently empty map.
    /// The committed `unsorted.osm.pbf` writes its way before its nodes.
    #[test]
    fn bbox_refuses_an_unsorted_pbf() {
        const UNSORTED_PBF: &str =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/unsorted.osm.pbf");
        assert!(
            std::path::Path::new(UNSORTED_PBF).exists(),
            "corpus fixture missing: {UNSORTED_PBF}. It is committed; rebuild from unsorted/unsorted.osm via \
             builder/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        // The box covers both nodes, so a sorted file would have kept the way.
        let bbox = Bbox::parse("7.79,47.98,7.81,48.0").expect("box");
        let Err(err) = ingest_osm(&sources(&[UNSORTED_PBF]), &cfg, Some(bbox), &quiet()) else {
            panic!("an unsorted .pbf must not be cropped silently");
        };
        assert!(err.contains("not sorted"), "unexpected message: {err}");
        // Without a box the ingest is order-agnostic (passes 1 and 2 are separate
        // reads), so the same file still packs — the refusal is scoped to --bbox.
        let ing =
            ingest_osm(&sources(&[UNSORTED_PBF]), &cfg, None, &quiet()).expect("uncropped ingest is order-agnostic");
        assert_eq!(ing.features.len(), 1, "the primary way survives without a box");
    }

    /// The same file listed twice is the sharpest duplicate case there is: every
    /// single object is a duplicate. If the merge is right, the result is exactly
    /// the one-source ingest — same features, same order, same POIs, same graph —
    /// and if it is wrong, everything is doubled.
    #[test]
    fn merging_a_source_with_itself_changes_nothing() {
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let once = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let twice = ingest_osm(&sources(&[TINY_PBF, TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        assert_eq!(shape(&once), shape(&twice), "a source merged with itself must be that source");

        // And the same with a box, which adds pass 0's id sets to the mix.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let once = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        let twice = ingest_osm(&sources(&[TINY_PBF, TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        assert_eq!(shape(&once), shape(&twice), "cropped, too");
    }

    /// Two halves of `tiny.osm` that overlap in the middle must ingest to exactly
    /// what the whole file ingests to — the real merge, not the degenerate one.
    /// The split is deliberately awkward: `tiny_west` holds R1 and the long ways,
    /// `tiny_east` holds R2 and repeats three of the shared objects, so the merge
    /// has to interleave two id runs *and* drop duplicates, not just concatenate.
    #[test]
    fn merging_two_overlapping_halves_rebuilds_the_whole() {
        const WEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_west.osm.pbf");
        const EAST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_east.osm.pbf");
        for f in [WEST, EAST] {
            assert!(
                std::path::Path::new(f).exists(),
                "corpus fixture missing: {f}. It is committed; rebuild via builder/tests/corpus/build_corpus.sh"
            );
        }
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let whole = ingest_osm(&sources(&[TINY_PBF]), &cfg, None, &quiet()).expect("ingest");
        let halves = ingest_osm(&sources(&[WEST, EAST]), &cfg, None, &quiet()).expect("ingest");
        assert_eq!(shape(&whole), shape(&halves), "west + east must rebuild tiny.osm exactly");

        // Cropped: pass 0's node phase has to finish across BOTH files before
        // either one's ways can be judged. W7/W7b start west and run east, so a
        // per-file selection would come out with a different set.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let whole = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        let halves = ingest_osm(&sources(&[WEST, EAST]), &cfg, Some(bbox), &quiet()).expect("ingest");
        assert_eq!(shape(&whole), shape(&halves), "west + east must rebuild the cropped tiny.osm exactly");
    }

    /// The tie-break, pinned: the **first** source that carries an id wins the
    /// whole object. `tiny_east` re-states way 107 as a `highway=track` (style 22,
    /// dropped by the preset's LOD table? no — it simply differs from primary=5),
    /// so listing it first changes the style and listing it second changes nothing.
    #[test]
    fn the_first_source_carrying_an_id_wins_it() {
        const WEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_west.osm.pbf");
        const EAST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny_east.osm.pbf");
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json")).expect("config");
        let style_of_107 = |paths: &[&str]| {
            let ing = ingest_osm(&sources(paths), &cfg, None, &quiet()).expect("ingest");
            // Way 107 is the only feature spanning lon 7.800..7.812 at lat 47.988.
            ing.features
                .iter()
                .find(|f| {
                    let (minx, miny, maxx, _) = f.geom.bounds();
                    (miny - 47.988).abs() < 1e-9 && (minx - 7.800).abs() < 1e-9 && (maxx - 7.812).abs() < 1e-9
                })
                .map(|f| f.style_id)
        };
        let west_first = style_of_107(&[WEST, EAST]).expect("way 107 kept");
        let east_first = style_of_107(&[EAST, WEST]).expect("way 107 kept");
        assert_ne!(west_first, east_first, "the two copies of way 107 must be distinguishable");
        assert_eq!(west_first, style_of_107(&[WEST]).expect("way 107"), "west first ⇒ west's copy");
        assert_eq!(east_first, style_of_107(&[EAST]).expect("way 107"), "east first ⇒ east's copy");

        // And on a node, where the loser is the copy carrying the tags: east's
        // node 25 is a drinking-water POI, west's is bare. Losing on the id means
        // losing the tags too, so with west first that POI does not exist.
        let pois = |paths: &[&str]| ingest_osm(&sources(paths), &cfg, None, &quiet()).expect("ingest").pois.len();
        assert_eq!(pois(&[EAST]), pois(&[WEST]) + 1, "only east's node 25 is a POI");
        assert_eq!(pois(&[WEST, EAST]), pois(&[WEST]), "west first ⇒ east's tagged copy contributes nothing");
        assert_eq!(pois(&[EAST, WEST]), pois(&[EAST]), "east first ⇒ its POI survives");
    }

    /// [`Keyed`] is the merge's whole ordering and dedup mechanism, so its two
    /// operations are pinned directly: `retain_keys` keeps order while dropping,
    /// and `sort` restores ascending-id order over interleaved source runs.
    #[test]
    fn keyed_retains_in_order_and_sorts_by_id() {
        let mut k = Keyed::new(true);
        for (id, name) in [(9_i64, "a"), (3, "b"), (11, "c"), (3, "dup"), (1, "d")] {
            k.push(id, name);
        }
        k.retain_keys(|id| id != 3);
        assert_eq!(k.items, ["a", "c", "d"], "retain preserves the surviving order");
        assert_eq!(k.keys, [9, 11, 1]);
        k.sort();
        assert_eq!(k.items, ["d", "a", "c"], "sort puts them in ascending id order");
        assert_eq!(k.keys, [1, 9, 11]);

        // Untagged (single source) is a plain Vec — nothing is recorded to sort by.
        let mut plain = Keyed::new(false);
        plain.push(9, "a");
        plain.push(3, "b");
        assert!(plain.keys.is_empty(), "a single source records no tags");
        assert_eq!(plain.into_items(), ["a", "b"], "and keeps file order");
    }

    /// [`IdSet`] is only correct if `freeze` runs between filling and querying —
    /// and `freeze` must be safe to call twice (pass 0 freezes the node set early).
    #[test]
    fn id_set_freezes_and_dedupes() {
        let mut s = IdSet::default();
        s.absorb(vec![9_i64, 3]);
        s.absorb(vec![9, -1, 3]);
        s.freeze();
        s.freeze();
        assert_eq!(s.len(), 3, "duplicates collapse");
        for id in [-1, 3, 9] {
            assert!(s.contains(id));
        }
        for id in [0, 4, 10] {
            assert!(!s.contains(id));
        }
    }

    fn tags(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    /// The closed-way polygon/line gate: `area=yes` forces area even with no
    /// AREA_TAGS key; `area=no` forces a line even with one present (the W12
    /// `natural=water area=no` case); absent `area` falls back to any AREA_TAGS key.
    #[test]
    fn is_area_overrides_and_tag_fallback() {
        assert!(is_area(&tags(&[("area", "yes")])), "area=yes ⇒ area regardless of other tags");
        assert!(!is_area(&tags(&[("area", "no"), ("natural", "water")])), "area=no ⇒ never an area");
        for key in AREA_TAGS {
            assert!(is_area(&tags(&[(key, "whatever")])), "AREA_TAGS key {key} ⇒ area");
        }
        assert!(!is_area(&tags(&[("highway", "residential")])), "no area tag, no AREA_TAGS key ⇒ line");
        // An unrecognized `area` value falls through to the tag fallback (not yes/no).
        assert!(!is_area(&tags(&[("area", "maybe")])), "unknown area value, no AREA_TAGS key ⇒ line");
        assert!(is_area(&tags(&[("area", "maybe"), ("building", "yes")])), "unknown area value falls back to tags");
    }
}
