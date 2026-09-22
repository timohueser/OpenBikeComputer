//! Read one or more `.osm.pbf` files into styled features: lines, closed-way polygons, and
//! multipolygon or `boundary` relation areas.
//!
//! Pass 1 builds the `node_id -> coord` store, matches tagged nodes against the POI table, and
//! collects qualifying area relations — relations sit last in a sorted PBF, so one read sees them
//! after the nodes. Pass 2 resolves ways into features and coastlines and captures the geometry of
//! any way a relation needs; [`assemble_multipolygon`] then builds the relation areas. Assembly is
//! additive: a tagged closed way that is also a relation member yields its own polygon and
//! contributes to the relation. A closed `highway=residential` loop is a line only, never a blob.
//!
//! Coordinates use `decimicro / 1e7`, never `* 1e-7`, so the f64 lon/lat match osmium's exactly.
//!
//! Given more than one source, every pass reads every source and the results are folded together.
//! On a duplicate the first source on the command line wins, decided on the `(type, id)` alone, so
//! the winner is a whole object and never a mix of two. The fold then restores ascending id order
//! per type, which is what a merged sorted file would have handed to the same pass: feature order
//! decides which quadtree chunk a feature lands in, and therefore the packed bytes. A single source
//! is left strictly alone, untagged and unsorted. Sources are read in parallel ([`par_sources`])
//! and the fold is sequential in command-line order, so the result never depends on which thread
//! finished first.
//!
//! A `--bbox` adds a pass 0 ([`select_crop`]) that reproduces `osmium extract --bbox` in-process,
//! as a renderer-aware variant of osmium's `smart` strategy. A way touching the box is kept whole,
//! with the outside nodes it needs, because [`resolve_coords`] drops a way with a missing node
//! rather than trimming it at the border, and because a nav edge must end where the way ends. The
//! member ways of a kept area relation are completed too, or a multipolygon would be dropped whole
//! as soon as one ring segment lay outside the box. Only area relations the active config can
//! render are completed, so a small crop never pulls in a continent-sized route relation.

use std::collections::{HashMap, HashSet};

use osmpbf::{Blob, BlobReader, BlobType, ByteOffset, Element, RelMemberType};
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

/// Coastlines are captured separately and always: they feed the bbox and the land/sea split.
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
    pub pois: Vec<Poi>,
    pub landmark_links: Vec<poi::LandmarkLink>,
    pub nav_graph: NavGraph,
}

/// A pass-1 area relation awaiting member geometry (pass 2) and assembly.
struct PendingRelation {
    style_id: u8,
    min_lod: usize,
    /// Member way ids in member order. Roles are dropped — `build_area` classifies outer and inner
    /// by geometry.
    member_ways: Vec<i64>,
}

/// The tags whose presence (with `area != no`) classifies a closed way as a polygon.
const AREA_TAGS: [&str; 6] = ["building", "landuse", "amenity", "leisure", "natural", "waterway"];

/// `decimicro / 1e7`, never `* 1e-7`, so coords match osmium exactly.
#[inline]
fn to_deg(decimicro: i32) -> f64 {
    decimicro as f64 / 1e7
}

/// A `--bbox` crop region, held in the PBF's own decimicro-degree integer grid — the fixed point
/// `osmium::Location` stores. [`Bbox::contains`] is then an integer comparison, so the in-process
/// crop cannot disagree with `osmium extract` about a node sitting a float ULP from the boundary.
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
    /// Parse a `W,S,E,N` degrees spec, as strictly as `osmium extract` parses its own `--bbox`:
    /// four finite in-range numbers, west strictly west of east and south strictly south of north.
    ///
    /// A box wrapping the antimeridian is rejected. Every stage downstream — the header bbox, the
    /// quadtree root box, the land clip — assumes `min < max` in plain degrees.
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

    /// The box back in degrees, snapped to the decimicro grid it was parsed onto. Handed to
    /// `osmium extract` on the multi-input merge path, so both croppers see the identical box.
    pub fn to_degrees(self) -> (f64, f64, f64, f64) {
        (to_deg(self.min_lon), to_deg(self.min_lat), to_deg(self.max_lon), to_deg(self.max_lat))
    }

    /// Inclusive integer microdegree coordinates contained in this box.
    pub fn microdegree_bounds(self) -> (i64, i64, i64, i64) {
        (
            (i64::from(self.min_lon) + 9).div_euclid(10),
            (i64::from(self.min_lat) + 9).div_euclid(10),
            i64::from(self.max_lon).div_euclid(10),
            i64::from(self.max_lat).div_euclid(10),
        )
    }

    /// Closed on all four edges, exactly like `osmium::Box::contains`.
    #[inline]
    fn contains(&self, lon: i32, lat: i32) -> bool {
        lon >= self.min_lon && lon <= self.max_lon && lat >= self.min_lat && lat <= self.max_lat
    }
}

/// The area of a `W,S,E,N` degree box on the sphere, in km². Pack time follows the region size far
/// more closely than the source file size does: the box decides how much survives ingest.
pub fn box_area_km2((w, s, e, n): (f64, f64, f64, f64)) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let lon_span = (e - w).to_radians();
    let lat_band = n.to_radians().sin() - s.to_radians().sin();
    EARTH_RADIUS_KM * EARTH_RADIUS_KM * lon_span * lat_band
}

/// The `W,S,E,N` box a source declares in its PBF header, if it declares one. It is the first blob
/// of the file, so the answer costs one read. A source without it is not an error.
pub fn declared_bbox(path: &str) -> Result<Option<(f64, f64, f64, f64)>, String> {
    let mut reader = BlobReader::from_path(path).map_err(|e| format!("open {path}: {e}"))?;
    let Some(blob) = reader.next() else { return Ok(None) };
    let blob = blob.map_err(|e| format!("read {path}: {e}"))?;
    match blob.decode().map_err(|e| format!("read {path}: {e}"))? {
        osmpbf::BlobDecode::OsmHeader(header) => {
            Ok(header.bbox().map(|b| (b.left, b.bottom, b.right, b.top)).filter(|(w, s, e, n)| w < e && s < n))
        }
        _ => Ok(None),
    }
}

/// What a blob scan should do after the element it was just handed.
enum Scan {
    Continue,
    /// Stop here. [`scan_blobs`] returns the offset of the blob this element came from, so a later
    /// pass can resume at exactly this point.
    StopAtThisBlob,
}

/// Stream a `.pbf`'s data blobs — from `start`, or from the beginning — handing every element to
/// `f`, and return the offset of the blob the scan stopped in (`None` if it ran to the end).
///
/// This is `ElementReader::for_each` plus the ability to stop and to resume. A sorted PBF stores
/// nodes, then ways, then relations, and the node section is about 85 % of the bytes, so a pass that
/// only wants ways skips straight to them. The blob boundary is also the ingest's cancellation
/// checkpoint, and every reading pass goes through here.
///
/// Blobs are decoded on the rayon pool a chunk at a time, but `f` is a stateful fold that must see
/// elements in file order, so the decoded blocks reach it one after another in that order and
/// memory stays bounded by the chunk. A chunk can overshoot a [`Scan::StopAtThisBlob`]: that work
/// is discarded unhandled, together with any read or decode error inside it, so a stopping scan
/// succeeds or fails exactly as a sequential one would.
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
    let chunk = chunk_len();
    let mut raw: Vec<Blob> = Vec::with_capacity(chunk);
    loop {
        // A read error ends the chunk but is only reported after the blobs before it are handled,
        // and not at all if the scan stops first — matching the lazy sequential reader.
        raw.clear();
        let mut read_err: Option<String> = None;
        while raw.len() < chunk {
            match reader.next() {
                // The header blob carries no elements; only OSMData blocks do.
                Some(Ok(blob)) if matches!(blob.get_type(), BlobType::OsmData) => raw.push(blob),
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    read_err = Some(format!("read {path}: {e}"));
                    break;
                }
                None => break,
            }
        }

        // Decode in parallel; an error surfaces below, only if the scan reaches the failing blob.
        progress.check()?;
        let blocks: Vec<_> = raw.par_iter().map(|b| (b.offset(), b.to_primitiveblock())).collect();

        for (offset, block) in blocks {
            progress.check()?;
            let block = block.map_err(|e| format!("decode {path}: {e}"))?;
            for el in block.elements() {
                if let Scan::StopAtThisBlob = f(el) {
                    return Ok(offset);
                }
            }
        }
        if let Some(e) = read_err {
            return Err(e);
        }
        if raw.len() < chunk {
            return Ok(None);
        }
    }
}

/// How many raw blobs one [`scan_blobs`] chunk holds: enough to keep every rayon worker busy
/// through a decode round, small enough that the in-flight blobs stay tens of megabytes.
fn chunk_len() -> usize {
    2 * rayon::current_num_threads().max(1)
}

/// Run `f` over every source in parallel, collecting the results in source order. Each pass reads
/// each file independently, and only the fold that combines them has to be ordered.
fn par_sources<T, F>(paths: &[String], f: F) -> Result<Vec<T>, String>
where
    T: Send,
    F: Fn(usize, &str) -> Result<T, String> + Sync,
{
    paths.par_iter().enumerate().map(|(i, p)| f(i, p.as_str())).collect()
}

/// Per-element output tagged with the id of the OSM object that produced it.
///
/// The tag is what lets several `.pbf`s be read independently and still come out as one merged file
/// would have produced them: later copies of an already-seen object dropped
/// ([`Keyed::retain_keys`]), everything back in id order ([`Keyed::sort`]). With a single source
/// nothing is tagged and this is a plain `Vec<T>`, so an uncropped country pack pays nothing.
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

    /// Drop every item whose key `keep` rejects, preserving order. Tagged only: it is a merge
    /// operation and never runs on a single-source ingest.
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

    /// Put the items back in ascending-id order — the order a merged, sorted `.pbf` would have
    /// handed them to the same pass.
    ///
    /// The sort is stable, so a file that repeats an id inside itself keeps its own order instead
    /// of picking one arbitrarily. The already-sorted check keeps the transient pair vector — the
    /// only copy of the payload this merge makes — out of the common case.
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

/// A grow-then-freeze set of OSM ids, backed by a sorted `Vec`: 8 flat bytes and a binary search
/// instead of a `HashSet`'s per-entry overhead, because these sets are the memory floor of a
/// `--bbox` run over a large source. Each set is filled in one pass and read in a later one, and
/// `contains` on an unfrozen set would silently lie, so freezing is the type's one rule.
#[derive(Default)]
struct IdSet(Vec<i64>);

impl IdSet {
    fn push(&mut self, id: i64) {
        self.0.push(id);
    }

    /// Take another source's ids wholesale, moving the first batch instead of copying it.
    fn absorb(&mut self, mut ids: Vec<i64>) {
        if self.0.is_empty() {
            self.0 = ids;
        } else {
            self.0.append(&mut ids);
        }
    }

    /// End the fill phase. Idempotent, so pass 0 can freeze the node set early (the first way
    /// needs it) and freeze the rest at the end.
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

/// The id sets that define a `--bbox` crop.
pub struct Crop {
    /// Nodes whose location falls inside the box.
    inside: IdSet,
    /// Nodes outside the box that a kept way still references — the halo that keeps
    /// boundary-crossing ways whole.
    halo: IdSet,
    /// Ways with at least one node inside the box, plus every member way of a renderable area
    /// relation touched by one of those ways.
    ways: IdSet,
    /// Renderable area relations reached from a way touching the box.
    relations: IdSet,
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

    #[inline]
    fn keeps_relation(&self, id: i64) -> bool {
        self.relations.contains(id)
    }

    /// Nothing inside the box and no way reaching into it — the caller should fail loudly rather
    /// than pack an empty map.
    fn is_empty(&self) -> bool {
        self.inside.len() == 0 && self.ways.len() == 0
    }
}

/// Pass 0 — select the crop across every source: nodes inside `bbox`, ways touching one of them,
/// complete members of renderable area relations reached by those ways, and the outside nodes all
/// selected ways still need. Also returns, per source, the offset of the first blob that holds a
/// way, so pass 2 resumes there instead of decoding the node section again.
///
/// The node phase has to finish across all sources before any file's ways can be judged: a way in
/// one file can have its only in-box node in another. The split costs nothing, because it falls
/// where the file is already split — phase A walks the node section and stops at the first
/// way-bearing blob, phase B resumes exactly there, phase C completes the area relations.
///
/// This is the one pass that needs the source type-sorted, because phase A stops at the first way.
/// An element out of that order is reported as an error rather than silently skipped.
fn select_crop(
    paths: &[String],
    bbox: Bbox,
    config: &Config,
    progress: &Progress,
) -> Result<(Crop, Vec<Option<ByteOffset>>), String> {
    progress.stage(Phase::Ingest, "Pass 0: selecting bbox...");
    // Phase A: the in-box node ids, from every source.
    let scans = par_sources(paths, |_, path| {
        let mut ids: Vec<i64> = Vec::new();
        let mut saw_relation = false;
        let mut out_of_order = false;
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
            // The first way: the node section is behind us and phase B takes over from this blob.
            // A file with no ways at all just runs to the end.
            Element::Way(_) => {
                out_of_order |= saw_relation;
                Scan::StopAtThisBlob
            }
            Element::Relation(_) => {
                saw_relation = true;
                Scan::Continue
            }
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a way follows a relation), so --bbox cannot select its areas — sort it \
                 first (e.g. `osmium sort`)"
            ));
        }
        Ok((ids, ways_at))
    })?;
    let mut inside = IdSet::default();
    let mut ways_at = Vec::with_capacity(paths.len());
    for (ids, at) in scans {
        inside.absorb(ids);
        ways_at.push(at);
    }
    inside.freeze();

    // Phase B: ways touching the box and their halo. Stop at the first relation-bearing blob, so
    // relation member lists never accumulate for the whole source.
    let scans = par_sources(paths, |i, path| {
        let (mut ways, mut halo) = (Vec::new(), Vec::new());
        let (mut saw_way, mut out_of_order) = (false, false);
        let relations_at = scan_blobs(path, ways_at[i], progress, |el| {
            match el {
                Element::Way(w) => {
                    saw_way = true;
                    if w.refs().any(|r| inside.contains(r)) {
                        ways.push(w.id());
                        // The halo: every other node this way needs. Ids already `inside` are
                        // skipped — `keeps_node` checks both sets, and a dense urban box would
                        // otherwise store most of its nodes twice.
                        for r in w.refs() {
                            if !inside.contains(r) {
                                halo.push(r);
                            }
                        }
                    }
                }
                // Nodes before the first way of the resume blob are ones phase A already saw;
                // anything after a way means the file is not sorted.
                Element::Node(_) | Element::DenseNode(_) => out_of_order |= saw_way,
                Element::Relation(_) => return Scan::StopAtThisBlob,
            }
            Scan::Continue
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a node follows a way), so --bbox cannot select its ways — sort it first \
                 (e.g. `osmium sort`)"
            ));
        }
        Ok((ways, halo, relations_at))
    })?;
    let (mut ways, mut halo) = (IdSet::default(), IdSet::default());
    let mut relations_at = Vec::with_capacity(scans.len());
    for (w, h, at) in scans {
        ways.absorb(w);
        halo.absorb(h);
        relations_at.push(at);
    }
    ways.freeze();

    // Phase C: complete every renderable area relation reached by a touching way. Relations are
    // streamed in source order to match the ingest merge rule: on a duplicate relation id the first
    // renderable copy wins.
    let touching_ways = ways.len();
    let mut seen_relations = HashSet::new();
    let mut relation_ways = IdSet::default();
    let mut relation_ids = IdSet::default();
    for (i, path) in paths.iter().enumerate() {
        let Some(at) = relations_at[i] else { continue };
        let mut saw_relation = false;
        let mut out_of_order = false;
        scan_blobs(path, Some(at), progress, |el| {
            match el {
                Element::Relation(r) => {
                    saw_relation = true;
                    if let Some(relation) = pending_relation(&r, config) {
                        if (paths.len() == 1 || seen_relations.insert(r.id()))
                            && relation.member_ways.iter().any(|&id| ways.contains(id))
                        {
                            relation_ids.push(r.id());
                            relation_ways.absorb(relation.member_ways);
                        }
                    }
                }
                // The resume blob can hold its final ways before its first relation. An object of
                // an earlier type after that relation is genuinely out of order.
                Element::Node(_) | Element::DenseNode(_) | Element::Way(_) => out_of_order |= saw_relation,
            }
            Scan::Continue
        })?;
        if out_of_order {
            return Err(format!(
                "{path} is not sorted (a node or way follows a relation), so --bbox cannot complete its areas — \
                 sort it first (e.g. `osmium sort`)"
            ));
        }
    }
    relation_ids.freeze();
    relation_ways.freeze();

    // Relation completion can introduce ways that never touch an in-box node. Read only the way
    // section again to collect every node those ways require.
    if relation_ways.len() != 0 {
        let scans = par_sources(paths, |i, path| {
            let mut relation_halo = Vec::new();
            scan_blobs(path, ways_at[i], progress, |el| {
                if let Element::Way(w) = el {
                    if relation_ways.contains(w.id()) {
                        for id in w.refs() {
                            if !inside.contains(id) {
                                relation_halo.push(id);
                            }
                        }
                    }
                }
                Scan::Continue
            })?;
            Ok(relation_halo)
        })?;
        for ids in scans {
            halo.absorb(ids);
        }
    }

    ways.absorb(relation_ways.0);
    ways.freeze();
    halo.freeze();
    progress.log(format!(
        "  {} node(s) in box, {} touching way(s) + {} relation member(s) from {} area relation(s) kept (+{} boundary node(s))",
        inside.len(),
        touching_ways,
        ways.len().saturating_sub(touching_ways),
        relation_ids.len(),
        halo.len()
    ));
    Ok((Crop { inside, halo, ways, relations: relation_ids }, ways_at))
}

/// One source's pass-1 harvest.
struct NodeScan {
    links: Keyed<poi::LandmarkLink>,
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
    /// Every way id this source processed, including the ones that produced nothing at all.
    /// Ownership is decided on the id alone, so an untagged or unresolvable copy here still has to
    /// shadow a later source's copy. Left empty for a single source, which claims everything.
    claimed: Vec<i64>,
}

/// What an ingest does with the routable ways it collected.
enum NavMode {
    /// Build the whole-extract nav graph and drop the ways: the ordinary pack.
    Graph,
    /// Keep the ways and build no graph: the cell cutter builds one graph per cell from them, so a
    /// whole-extract graph would be wasted work whose island pruning is the wrong shape for a cell.
    KeepWays,
}

/// Two-pass ingest of one or more `.osm.pbf`s, merged as described in the module docs. `bbox` crops
/// the inputs to a box first, in a third, id-only pass.
pub fn ingest_osm(
    paths: &[String],
    config: &Config,
    bbox: Option<Bbox>,
    progress: &Progress,
) -> Result<Ingested, String> {
    ingest_inner(paths, config, bbox, progress, NavMode::Graph).map(|(ing, _)| ing)
}

/// [`ingest_osm`], but returning the routable ways instead of a built nav graph — what
/// [`crate::cut`] needs, because a cell classifies junctions from the whole way set and cuts the
/// ways itself at the cell edges. The returned [`Ingested`] carries an empty `nav_graph`.
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
    // More than one source means every output is tagged with the id that produced it, which is
    // what the fold needs to drop duplicates and restore a single merged file's order.
    let merging = paths.len() > 1;
    if merging {
        progress.stage(
            Phase::Merging,
            format!("Merging {} sources (on a duplicate id, the first source wins)...", paths.len()),
        );
    }

    // Pass 0 (only with --bbox): the id selection, plus the per-source offset where the ways begin.
    let (crop, ways_at) = match bbox {
        Some(bb) => {
            let (crop, ways_at) = select_crop(paths, bb, config, progress)?;
            if crop.is_empty() {
                let (w, s, e, n) = bb.to_degrees();
                return Err(format!("--bbox {w},{s},{e},{n} does not overlap any data in {}", paths.join(", ")));
            }
            (Some(crop), ways_at)
        }
        None => (None, vec![None; paths.len()]),
    };

    // Pass 1: node-location store and relation collection, per source. The stage strings reach the
    // build UI, so each is reported when its pass actually starts, not both up front.
    progress.stage(Phase::Ingest, "Pass 1: reading nodes...");
    let scans = par_sources(paths, |_, path| read_nodes(path, config, crop.as_ref(), merging, progress))?;
    let NodeScan { nodes, pois: node_pois, rels, links } = fold_node_scans(scans, merging);
    let pending = rels.into_items();
    let needed_ways: HashSet<i64> = pending.iter().flat_map(|r| r.member_ways.iter().copied()).collect();

    // Pass 2: ways into features and coastlines, plus member-way geometry capture.
    progress.stage(Phase::Ingest, "Pass 2: processing ways...");
    let scans = par_sources(paths, |i, path| {
        read_ways(path, ways_at[i], config, crop.as_ref(), &nodes, &needed_ways, merging, progress)
    })?;
    let WayScan { features, coastlines, pois: way_pois, routable, member_geom, .. } = fold_way_scans(scans, merging);
    let mut features = features.into_items();
    let coastlines = coastlines.into_items();
    let routable_ways = routable.into_items();
    // POI candidates from both passes, deduped after assembly — node candidates first, then way
    // centroids, the order a single sorted file produces them in.
    let mut poi_cands = node_pois.into_items();
    poi_cands.extend(way_pois.into_items());

    // Assemble relation areas from the captured member geometry: each outer ring plus its nested
    // holes becomes one polygon, styled by the relation. Like osmium, assemble only when ALL member
    // ways are present; an incomplete relation is dropped rather than assembled from survivors,
    // which would emit a phantom boundary-crossing polygon.
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

    // POIs: collapse OSM double-mapping, then log per-category counts.
    let (mut pois, poi_dropped) = poi::dedupe(poi_cands);
    poi::resolve_approaches(&mut pois, &routable_ways, &config.routing.profiles);
    let mut landmark_links: Vec<_> =
        pois.iter().filter(|p| p.wikidata.is_some() || p.wikipedia.is_some()).map(poi::LandmarkLink::from).collect();
    landmark_links.extend(links.into_items());
    pois.retain(|p| p.subtype != 0);
    progress.log(poi::format_counts(&pois, poi_dropped));

    // Nav graph: junctions and deduped edges from the routable ways, then island pruning and the
    // edge splits the format guarantees.
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

    Ok((Ingested { features, coastlines, pois, landmark_links, nav_graph }, kept_ways))
}

/// Pass 1, one source: node-location store, node POIs and area relations.
///
/// Cropped, this keeps only the nodes the extract would contain, halo included, so a tagged node
/// just outside the box that a kept way needs becomes a POI here exactly as it would in an `osmium
/// extract` output. Cropped runs collect only the relations pass 0 selected; the
/// all-members-present rule below still rejects relations already incomplete at the source
/// extract's own edge.
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
    let mut links = Keyed::new(tagged);
    let keeps_node = |id: i64| crop.is_none_or(|c| c.keeps_node(id));
    let keeps_relation = |id: i64| crop.is_none_or(|c| c.keeps_relation(id));
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
            Element::Relation(r) => {
                if keeps_relation(r.id()) {
                    collect_relation(&r, config, &mut rels);
                }
                let tags: HashMap<_, _> = r.tags().collect();
                if tags.contains_key("wikidata") || tags.contains_key("wikipedia") {
                    links.push(
                        r.id(),
                        poi::LandmarkLink {
                            metadata: obc_formats::obcm::PoiMetadata {
                                source: obc_formats::obcm::SourceId::osm(3, r.id() as u64),
                                approach: None,
                            },
                            position: None,
                            wikidata: tags.get("wikidata").map(|value| (*value).into()),
                            wikipedia: tags.get("wikipedia").map(|value| (*value).into()),
                            hours: tags.get("opening_hours").and_then(|value| hours::parse(value)),
                        },
                    );
                }
            }
            _ => {}
        }
        Scan::Continue
    })
    .map_err(|e| format!("pass 1: {e}"))?;
    Ok(NodeScan { nodes, pois, rels, links })
}

/// Combine the sources' pass-1 harvests, in command-line order.
fn fold_node_scans(scans: Vec<NodeScan>, merging: bool) -> NodeScan {
    let mut it = scans.into_iter();
    let mut acc = it.next().expect("at least one source");
    let mut seen_rels: HashSet<i64> = acc.rels.keys.iter().copied().collect();
    let mut seen_links: HashSet<i64> = acc.links.keys.iter().copied().collect();
    for mut next in it {
        // Ownership is tested BEFORE this source's nodes land in `acc`, so the question is whether
        // an earlier source already had this node — and the whole object loses, tags and all.
        next.pois.retain_keys(|id| !acc.nodes.contains_key(&id));
        next.rels.retain_keys(|id| seen_rels.insert(id));
        next.links.retain_keys(|id| seen_links.insert(id));
        acc.links.append(next.links);
        acc.pois.append(next.pois);
        acc.rels.append(next.rels);
        for (id, coord) in next.nodes {
            acc.nodes.entry(id).or_insert(coord);
        }
    }
    if merging {
        acc.pois.sort();
        acc.rels.sort();
        acc.links.sort();
    }
    acc
}

/// Pass 2, one source: ways into features, coastlines, POIs and routable topology, plus the geometry
/// of any way a relation needs.
///
/// `ways_at` is where pass 0 found this file's first way; starting there skips re-decoding the node
/// section. Without a `--bbox` there is no offset and the scan starts at the beginning, which is
/// also what keeps an uncropped ingest order-agnostic.
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
    // Routable-way topology for the nav graph. The OSM node ids are kept here (the render path
    // drops them) so shared nodes can be recovered as junctions after the pass.
    let mut routable = Keyed::new(tagged);
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    let mut claimed: Vec<i64> = Vec::new();
    let keeps_way = |id: i64| crop.is_none_or(|c| c.keeps_way(id));
    scan_blobs(path, ways_at, progress, |el| {
        if let Element::Way(w) = el {
            if keeps_way(w.id()) {
                // Claimed on sight, before anything can go wrong with it: a way this source could
                // not resolve still shadows a later copy.
                if tagged {
                    claimed.push(w.id());
                }
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way, as osmium's `InvalidLocationError` would.
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

/// Combine the sources' pass-2 harvests, in command-line order: a later source contributes only the
/// ways no earlier source claimed, and the survivors go back in way-id order.
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

/// Capture a routable way's node-id sequence and µdeg coords for the nav graph. Routability is
/// tag-based ([`nav::is_routable`]) and independent of styling. `coords` is snapped here to the µdeg
/// grid POIs and the serializer use, so edge lengths and later serialization agree.
fn push_routable_way(id: i64, w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], out: &mut Keyed<RoutableWay>) {
    if refs.len() < 2 {
        return;
    }
    // Classify once (routability plus the way-kind byte). This is the only place tags exist, so
    // the kind is captured here or never.
    let Some(kind) = nav::classify(w.tags()) else { return };
    let coords_udeg = coords.iter().map(|&(x, y)| (poi::to_udeg(x), poi::to_udeg(y))).collect();
    out.push(id, RoutableWay { node_ids: refs.to_vec(), coords: coords_udeg, kind });
}

/// Classify one node's tags against the POI table; push a candidate on match.
fn push_node_poi<'a, I>(id: i64, tags: I, decimicro_lon: i32, decimicro_lat: i32, out: &mut Keyed<Poi>)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let tags: Vec<_> = tags.into_iter().collect();
    if let Some(poi::Classification { subtype, name, raw_hours, elevation_m, population }) =
        poi::classify_linked(tags.iter().copied())
    {
        out.push(
            id,
            Poi {
                metadata: obc_formats::obcm::PoiMetadata {
                    source: obc_formats::obcm::SourceId::osm(1, id as u64),
                    approach: None,
                },
                access_nodes: vec![id],
                wikidata: tags.iter().find(|(k, _)| *k == "wikidata").map(|(_, v)| (*v).into()),
                wikipedia: tags.iter().find(|(k, _)| *k == "wikipedia").map(|(_, v)| (*v).into()),
                subtype,
                lon_udeg: poi::to_udeg(to_deg(decimicro_lon)),
                lat_udeg: poi::to_udeg(to_deg(decimicro_lat)),
                name,
                from_node: true,
                hours: raw_hours.and_then(hours::parse),
                elevation_m,
                population,
            },
        );
    }
}

/// Resolve a way's node refs to degree coordinates. `None` if any node is missing, and the caller
/// then drops the way (osmium's `InvalidLocationError`).
fn resolve_coords(refs: &[i64], nodes: &HashMap<i64, (i32, i32)>) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for r in refs {
        let &(dx, dy) = nodes.get(r)?;
        coords.push((to_deg(dx), to_deg(dy)));
    }
    Some(coords)
}

/// Classify a `type=multipolygon` or `type=boundary` relation (skipping `admin_level`) for area
/// assembly. Shared by crop selection and pass 1, so the crop completes exactly the relations the
/// renderer can consume.
fn pending_relation(r: &osmpbf::Relation, config: &Config) -> Option<PendingRelation> {
    let tags: HashMap<&str, &str> = r.tags().collect();
    match tags.get("type").copied() {
        Some("multipolygon") | Some("boundary") => {}
        _ => return None,
    }
    // admin_level relations are line-only → no polygon.
    if tags.contains_key("admin_level") {
        return None;
    }
    let style = config.get_style(&tags)?;
    let member_ways: Vec<i64> =
        r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
    if member_ways.is_empty() {
        return None;
    }
    Some(PendingRelation { style_id: style.id, min_lod: style.min_lod, member_ways })
}

/// Collect a renderable area relation for pass-2 assembly. Roles are ignored; non-way members are
/// skipped.
fn collect_relation(r: &osmpbf::Relation, config: &Config, pending: &mut Keyed<PendingRelation>) {
    if let Some(relation) = pending_relation(r, config) {
        pending.push(r.id(), relation);
    }
}

/// One way: capture the coastline always, then style and classify into a single polygon-or-line
/// emission. `refs` and `coords` are pre-resolved.
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

    // Coastlines are captured ALWAYS — even if the way is also closed and styled — and as lines.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(w.id(), coords.to_vec());
    }

    // A closed way matching the POI table yields a POI at the ring centroid, independent of
    // styling: a bare `shop=supermarket` outline has no style at all. Relations are out of scope.
    if is_closed || tags.contains_key("wikidata") || tags.contains_key("wikipedia") {
        if let Some(poi::Classification { subtype, name, raw_hours, elevation_m, population }) =
            poi::classify_linked(tags.iter().map(|(&k, &v)| (k, v)))
                .filter(|p| p.subtype != obc_formats::obcm::SUMMIT_SUBTYPE_ID)
        {
            let (cx, cy) = if is_closed { poi::ring_centroid(coords) } else { coords[0] };
            let subtype = if is_closed { subtype } else { 0 };
            pois.push(
                w.id(),
                Poi {
                    metadata: obc_formats::obcm::PoiMetadata {
                        source: obc_formats::obcm::SourceId::osm(2, w.id() as u64),
                        approach: None,
                    },
                    access_nodes: refs.to_vec(),
                    wikidata: tags.get("wikidata").map(|v| (*v).into()),
                    wikipedia: tags.get("wikipedia").map(|v| (*v).into()),
                    subtype,
                    lon_udeg: poi::to_udeg(cx),
                    lat_udeg: poi::to_udeg(cy),
                    name,
                    from_node: false,
                    hours: raw_hours.and_then(hours::parse),
                    elevation_m,
                    population,
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
        // Skip rings osmium's assembler would reject as invalid, such as a self-intersecting
        // building: no polygon and no line.
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

/// Closed-way area heuristic: `area=yes` is an area, `area=no` never is, otherwise it is an area iff
/// it carries any [`AREA_TAGS`] key.
fn is_area(tags: &HashMap<&str, &str>) -> bool {
    // A cliff can form a closed rim, but it still marks an edge, not a filled area.
    if tags.get("natural") == Some(&"cliff") {
        return false;
    }
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

    fn sources(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    fn quiet() -> Progress {
        Progress::silent()
    }

    /// Everything an ingest produced, flattened into one comparable value — enough to say "these
    /// two runs are the same map", including order, which decides the packed bytes downstream.
    fn shape(ing: &Ingested) -> Vec<String> {
        let mut out: Vec<String> =
            ing.features.iter().map(|f| format!("F {} {} {:?}", f.style_id, f.min_lod, f.geom.bounds())).collect();
        out.extend(ing.coastlines.iter().map(|c| format!("C {c:?}")));
        out.extend(ing.pois.iter().map(|p| format!("P {} {} {} {:?}", p.subtype, p.lon_udeg, p.lat_udeg, p.name)));
        out.push(format!("nav {} nodes, {} edges", ing.nav_graph.nodes.len(), ing.nav_graph.edges.len()));
        out
    }

    /// The `tiny.osm` truth table: relations assembled (R1's lake with a hole, R2's two forest
    /// outers) plus lines and closed-way polygons, giving 10 features.
    #[test]
    fn tiny_truth_table() {
        // The fixture is committed in-repo; a missing one is a hard failure, not a skip.
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

        assert_eq!(n(50, true), 3, "W5 closed forest + R2's two outer rings ⇒ 3 polygons");
        assert_eq!(n(36, true), 1, "R1 natural=water ⇒ 1 polygon (lake)");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(63, false), 1, "W9 admin_level=2 ⇒ 1 line");
        assert_eq!(n(36, false), 1, "W12 natural=water area=no ⇒ 1 line");

        // R1 is a lake WITH an island (one hole).
        let lake = ing.features.iter().find(|f| f.style_id == 36 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "R1 has one hole"),
            _ => unreachable!(),
        }

        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        // 5 polygons (3 forest, 1 pedestrian, 1 water lake) + 5 lines.
        assert_eq!(ing.features.len(), 10, "10 features total");
    }

    /// End-to-end POI extraction over the hand-authored `poi.osm` fixture, whose header comment is
    /// the truth table: node and closed-way classification, name folding, and both dedup pairs.
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

        // 16 candidates (13 nodes + 3 way-centroids), 2 dedup-dropped ⇒ 14 kept.
        assert_eq!(ing.pois.len(), 16, "distinct OSM identities survive");

        let find = |name: Option<&str>, subtype: u8| {
            ing.pois
                .iter()
                .find(|p| p.subtype == subtype && p.name.as_deref() == name)
                .unwrap_or_else(|| panic!("missing poi subtype {subtype} name {name:?}: {:?}", ing.pois))
        };

        // N1: named water node, exact µdeg grid.
        let n1 = find(Some("Marktbrunnen"), 1);
        assert_eq!((n1.lat_udeg, n1.lon_udeg, n1.from_node), (47_995_000, 7_850_000, true));
        // The node and building retain separate source identities.
        let n2 = find(Some("Edeka Mueller"), 13);
        assert_eq!((n2.lat_udeg, n2.lon_udeg, n2.from_node), (47_989_900, 7_859_900, true));
        // N3: CJK name folded to empty ⇒ unnamed.
        let n3 = find(None, 1);
        assert_eq!((n3.lat_udeg, n3.lon_udeg), (47_980_000, 7_840_000));
        // Nearby service identities stay distinct.
        find(Some("Brunnen A"), 1);
        assert!(ing.pois.iter().any(|p| p.subtype == 2), "the separate spring remains available");
        // W2: unnamed campsite way ⇒ POI at the ring centroid.
        let w2 = find(None, 5);
        assert_eq!((w2.lat_udeg, w2.lon_udeg, w2.from_node), (48_000_200, 7_870_200, false));
        // N4 (amenity=parking) never classified.
        assert_eq!(crate::poi::format_counts(&ing.pois, 0).matches("water 4").count(), 1);

        // The settlement rows: one of each class, the fall-backs, and the area centroid.
        let city = find(Some("Testville"), 21);
        assert_eq!(city.population, Some(250_000));
        assert_eq!(find(Some("Kleinstadt"), 22).population, None);
        find(Some("Grüßau"), 23);
        find(Some("A very long settlement n"), 24);
        find(Some("Tokyo"), 23);
        find(Some("Baeckerdorf"), 15);
        assert_eq!(find(Some("Freiburg"), 21).population, Some(220_286), "a shorter short_name is stored");
        let ring = find(Some("Ringdorf"), 23);
        assert_eq!((ring.lat_udeg, ring.lon_udeg, ring.from_node), (47_950_200, 7_900_200, false));
        find(Some("Mirnyy"), 23);
        assert_eq!(crate::poi::format_counts(&ing.pois, 0).matches("settlement 8").count(), 1);
    }

    #[test]
    fn bbox_parse_is_strict_about_the_box() {
        let ok = Bbox::parse("7.39,43.71,7.47,43.77").expect("valid box");
        assert_eq!(ok.to_degrees(), (7.39, 43.71, 7.47, 43.77), "degrees survive the decimicro round trip");
        assert_eq!(Bbox::parse(" 7.39 , 43.71 , 7.47 , 43.77 ").expect("whitespace"), ok, "fields are trimmed");
        assert_eq!(
            Bbox::parse("-8.0000011,-1.0000001,8.000001,1.0000001").unwrap().microdegree_bounds(),
            (-8_000_001, -1_000_000, 8_000_001, 1_000_000)
        );
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

    #[test]
    fn box_area_shrinks_with_latitude() {
        let one_degree_at_equator = box_area_km2((0.0, 0.0, 1.0, 1.0));
        assert!((one_degree_at_equator - 12_363.0).abs() < 10.0, "{one_degree_at_equator} km²");
        let one_degree_at_sixty = box_area_km2((0.0, 59.5, 1.0, 60.5));
        assert!((one_degree_at_sixty - 6_182.0).abs() < 10.0, "{one_degree_at_sixty} km²");

        // The Grimsel fixture box: a region a look is meant to reach.
        let grimsel = box_area_km2((8.15034, 46.48261, 8.46007, 46.72070));
        assert!((600.0..700.0).contains(&grimsel), "{grimsel} km²");
    }

    #[test]
    fn a_source_without_a_declared_box_reads_as_unknown() {
        assert_eq!(declared_bbox(TINY_PBF), Ok(None));
    }

    /// The relation-complete crop, over the `tiny.osm` truth table. The box covers R1 whole, takes
    /// only one of R2's two outer rings, and clips the middle of both open highways. Ways stay
    /// whole: W7b reaches far outside the box because one of its nodes is inside. Relations stay
    /// whole: R2's in-box W3 pulls in its outside W4 member, so both forest outers assemble.
    #[test]
    fn bbox_crop_keeps_ways_whole_and_completes_area_relations() {
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
        assert_eq!(n(36, true), 1, "R1 lake survives whole");
        let lake = ing.features.iter().find(|f| f.style_id == 36 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "island hole kept"),
            _ => unreachable!(),
        }
        // R2 touches the box through W3, so W4 is pulled in and both disjoint
        // outer rings survive. W5 is an unrelated closed forest outside the box.
        assert_eq!(n(50, true), 2, "R2's complete two-outer forest survives");
        // Out of the box entirely: W5/W6/W11 (lat ≥ 47.996), W9 (48.000), W8 coast.
        assert_eq!(n(15, true), 0, "W11 pedestrian area is north of the box");
        assert_eq!(n(12, false), 0, "W6 residential loop is north of the box");
        assert_eq!(n(63, false), 0, "W9 admin line is north of the box");
        assert!(ing.coastlines.is_empty(), "W8 coastline sits east of the box");
        // Kept: W7 primary, W7b trunk, W12 water line, R1's polygon, and both
        // of R2's forest polygons.
        assert_eq!(n(5, false), 1, "W7 primary crosses the east edge and is kept");
        assert_eq!(n(3, false), 1, "W7b trunk crosses the east edge and is kept");
        assert_eq!(n(36, false), 1, "W12 water line is inside");
        assert_eq!(ing.features.len(), 6, "1 lake + 2 forest polygons + 3 lines");

        // The headline: the trunk is not trimmed at the box edge (lon 7.809) — it keeps its far
        // node at 7.855, exactly as `osmium extract` would emit it.
        let trunk = ing.features.iter().find(|f| f.style_id == 3).expect("trunk line");
        let (_, _, maxx, _) = trunk.geom.bounds();
        assert!((maxx - 7.855).abs() < 1e-9, "trunk must reach its real end at 7.855, got {maxx}");

        let outside_forest = ing
            .features
            .iter()
            .filter(|f| f.style_id == 50 && is_polygon(&f.geom))
            .map(|f| f.geom.bounds().2)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            outside_forest > 7.811,
            "R2's member outside the 7.809 crop edge must be present, got max lon {outside_forest}"
        );
    }

    /// A box that swallows the whole file must change nothing — the crop path is
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

    /// Pass 0 is the one place that needs the PBF type-sorted, and a file that is not would
    /// otherwise select nothing at all and pack a silently empty map. The committed
    /// `unsorted.osm.pbf` writes its way before its nodes.
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
        // Without a box the ingest is order-agnostic, so the same file still packs.
        let ing =
            ingest_osm(&sources(&[UNSORTED_PBF]), &cfg, None, &quiet()).expect("uncropped ingest is order-agnostic");
        assert_eq!(ing.features.len(), 1, "the primary way survives without a box");
    }

    /// The same file listed twice is the sharpest duplicate case there is: every single object is a
    /// duplicate, so a right merge gives exactly the one-source ingest and a wrong one doubles it.
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

    /// Two halves of `tiny.osm` that overlap in the middle must ingest to exactly what the whole
    /// file does. The split is awkward on purpose: `tiny_west` holds R1 and the long ways,
    /// `tiny_east` holds R2 and repeats three shared objects, so the merge has to interleave two id
    /// runs and drop duplicates, not just concatenate.
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

        // Cropped: pass 0's node phase has to finish across BOTH files before either one's ways
        // can be judged. W7/W7b start west and run east, so a per-file selection would differ.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let whole = ingest_osm(&sources(&[TINY_PBF]), &cfg, Some(bbox), &quiet()).expect("ingest");
        let halves = ingest_osm(&sources(&[WEST, EAST]), &cfg, Some(bbox), &quiet()).expect("ingest");
        assert_eq!(shape(&whole), shape(&halves), "west + east must rebuild the cropped tiny.osm exactly");
    }

    /// The tie-break: the first source that carries an id wins the whole object. `tiny_east`
    /// re-states way 107 with a different style, so listing it first changes the style and listing
    /// it second changes nothing.
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

        // And on a node, where the loser is the copy carrying the tags: east's node 25 is a
        // drinking-water POI and west's is bare, so with west first that POI does not exist.
        let pois = |paths: &[&str]| ingest_osm(&sources(paths), &cfg, None, &quiet()).expect("ingest").pois.len();
        assert_eq!(pois(&[EAST]), pois(&[WEST]) + 1, "only east's node 25 is a POI");
        assert_eq!(pois(&[WEST, EAST]), pois(&[WEST]), "west first ⇒ east's tagged copy contributes nothing");
        assert_eq!(pois(&[EAST, WEST]), pois(&[EAST]), "east first ⇒ its POI survives");
    }

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

    /// `freeze` must be safe to call twice, because pass 0 freezes the node set early.
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

    /// The closed-way polygon/line gate: `area=yes` forces an area even with no AREA_TAGS key,
    /// `area=no` forces a line even with one present, and an absent `area` falls back to the keys.
    #[test]
    fn is_area_overrides_and_tag_fallback() {
        assert!(is_area(&tags(&[("area", "yes")])), "area=yes ⇒ area regardless of other tags");
        assert!(!is_area(&tags(&[("area", "no"), ("natural", "water")])), "area=no ⇒ never an area");
        assert!(!is_area(&tags(&[("natural", "cliff")])), "a closed cliff remains a line");
        for key in AREA_TAGS {
            assert!(is_area(&tags(&[(key, "whatever")])), "AREA_TAGS key {key} ⇒ area");
        }
        assert!(!is_area(&tags(&[("highway", "residential")])), "no area tag, no AREA_TAGS key ⇒ line");
        // An unrecognized `area` value falls through to the tag fallback (not yes/no).
        assert!(!is_area(&tags(&[("area", "maybe")])), "unknown area value, no AREA_TAGS key ⇒ line");
        assert!(is_area(&tags(&[("area", "maybe"), ("building", "yes")])), "unknown area value falls back to tags");
    }
}
