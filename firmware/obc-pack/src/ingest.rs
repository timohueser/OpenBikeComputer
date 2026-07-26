//! `ingest.rs` — read an `.osm.pbf` into styled features (lines, closed-way
//! polygons, and multipolygon/`boundary` relation areas). Two `osmpbf` passes —
//! three with a `--bbox`, which prepends a **pass 0** (see the cropping section
//! at the end of this doc):
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
//!   committed fixtures (`firmware/obc-sim/assets/repack.sh`) were packed from
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

use osmpbf::{Element, ElementReader, RelMemberType};

use crate::config::Config;
use crate::geom::{assemble_multipolygon, polygon_is_valid, Geom};
use crate::hours;
use crate::nav::{self, NavGraph, RoutableWay};
use crate::poi::{self, Poi};

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
    #[inline]
    fn insert(&mut self, id: i64) {
        self.0.push(id);
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

/// **Pass 0** — select the crop: nodes inside `bbox`, ways touching one of them,
/// and the outside nodes those ways still need.
///
/// One sweep suffices because a PBF is type-sorted (nodes, then ways, then
/// relations), so the node set is complete — and can be frozen — the moment the
/// first way arrives. Passes 1 and 2 don't care about order (they are separate
/// reads and relations only carry ids), so this is the one place that does; a
/// node arriving *after* a way therefore has to be an error rather than a
/// quietly wrong crop.
fn select_crop(pbf_path: &str, bbox: Bbox) -> Result<Crop, String> {
    println!("Pass 0: selecting bbox...");
    let mut inside = IdSet::default();
    let mut halo = IdSet::default();
    let mut ways = IdSet::default();
    let mut nodes_done = false;
    let mut out_of_order = false;
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| match el {
            Element::Node(n) => {
                out_of_order |= nodes_done;
                if bbox.contains(n.decimicro_lon(), n.decimicro_lat()) {
                    inside.insert(n.id());
                }
            }
            Element::DenseNode(n) => {
                out_of_order |= nodes_done;
                if bbox.contains(n.decimicro_lon(), n.decimicro_lat()) {
                    inside.insert(n.id());
                }
            }
            Element::Way(w) => {
                if !nodes_done {
                    inside.freeze();
                    nodes_done = true;
                }
                if w.refs().any(|r| inside.contains(r)) {
                    ways.insert(w.id());
                    // The halo: every other node this way needs. Ids already
                    // `inside` are skipped — `keeps_node` checks both sets, and a
                    // dense urban box would otherwise store most of its nodes twice.
                    for r in w.refs() {
                        if !inside.contains(r) {
                            halo.insert(r);
                        }
                    }
                }
            }
            Element::Relation(_) => {}
        })
        .map_err(|e| format!("pass 0 {pbf_path}: {e}"))?;
    if out_of_order {
        return Err(format!(
            "{pbf_path} is not sorted (a node follows a way), so --bbox cannot select ways in one pass — sort it \
             first with `osmium sort`"
        ));
    }
    // A file with no ways at all never hit the way branch.
    if !nodes_done {
        inside.freeze();
    }
    halo.freeze();
    ways.freeze();
    println!("  {} node(s) in box, {} way(s) kept (+{} boundary node(s))", inside.len(), ways.len(), halo.len());
    Ok(Crop { inside, halo, ways })
}

/// Two-pass ingest of a single `.osm.pbf` (lines + closed-way polygons +
/// relation-assembled area polygons). `bbox` crops the input to a box first (a
/// third, id-only pass; see the module docs).
pub fn ingest_osm(pbf_path: &str, config: &Config, bbox: Option<Bbox>) -> Result<Ingested, String> {
    // --- Pass 0 (only with --bbox): the `complete_ways` id selection. ---
    let crop = match bbox {
        Some(bb) => {
            let crop = select_crop(pbf_path, bb)?;
            if crop.is_empty() {
                let (w, s, e, n) = bb.to_degrees();
                return Err(format!("--bbox {w},{s},{e},{n} does not overlap any data in {pbf_path}"));
            }
            Some(crop)
        }
        None => None,
    };
    let keeps_node = |id: i64| crop.as_ref().is_none_or(|c| c.keeps_node(id));
    let keeps_way = |id: i64| crop.as_ref().is_none_or(|c| c.keeps_way(id));

    // --- Pass 1: node-location store + relation collection. ---
    // The PBF is node-sorted, so the store is filled before any relation is read.
    // The stage strings are matched by the web builder's progress UI — print each
    // when its pass actually starts, not both up front.
    println!("Pass 1: reading nodes...");
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut pending: Vec<PendingRelation> = Vec::new();
    let mut needed_ways: HashSet<i64> = HashSet::new();
    // POI candidates from both passes, deduped after assembly. Classification is
    // config-free (hardcoded table — locked decision on #115).
    let mut poi_cands: Vec<Poi> = Vec::new();
    // Cropped: only the nodes the extract would contain — which includes the halo,
    // so a tagged node just outside the box that a kept way needs becomes a POI
    // here exactly as it would in an `osmium extract` output (osmium writes those
    // nodes whole, tags and all). Matching that is the point.
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| match el {
            Element::Node(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut poi_cands);
            }
            Element::DenseNode(n) if keeps_node(n.id()) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut poi_cands);
            }
            // Relations are collected unfiltered even when cropping: the
            // all-members-present rule below already drops exactly the ones
            // osmium's crop would have left out (module docs).
            Element::Relation(r) => collect_relation(&r, config, &mut pending, &mut needed_ways),
            _ => {}
        })
        .map_err(|e| format!("pass 1 {pbf_path}: {e}"))?;

    // --- Pass 2: ways → features + coastlines, plus member-way geometry capture. ---
    println!("Pass 2: processing ways...");
    let mut features = Vec::new();
    let mut coastlines = Vec::new();
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::with_capacity(needed_ways.len());
    // Routable-way topology for the nav graph ([`crate::nav`]). We keep the OSM node
    // ids here (which the render path drops) so shared nodes can be recovered as
    // junctions after the pass; the graph is built from these once all ways are seen.
    let mut routable_ways: Vec<RoutableWay> = Vec::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| {
            if let Element::Way(w) = el {
                if !keeps_way(w.id()) {
                    return;
                }
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way — osmium would raise
                // `InvalidLocationError` here, and the way is dropped.
                let Some(coords) = resolve_coords(&refs, &nodes) else { return };
                push_routable_way(&w, &refs, &coords, &mut routable_ways);
                process_way(&w, &refs, &coords, config, &mut features, &mut coastlines, &mut poi_cands);
                if needed_ways.contains(&w.id()) {
                    member_geom.insert(w.id(), coords);
                }
            }
        })
        .map_err(|e| format!("pass 2 {pbf_path}: {e}"))?;

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
    println!("{}", poi::format_counts(&pois, poi_dropped));

    // --- Nav graph: junctions + deduped edges from the routable ways, then
    // island pruning (`routing.min_component_edges`) + v9-guarantee edge splits
    // ([`nav::build_graph_with`]). Serialized into the §8 nav section. Logged (with
    // component + kinds stats) alongside POIs.
    let (nav_graph, nav_stats) = nav::build_graph_with(&routable_ways, config.routing.min_component_edges);
    println!("{}", nav::format_summary(&nav_graph, &nav_stats));

    Ok(Ingested { features, coastlines, pois, nav_graph })
}

/// Capture a routable way's node-id sequence + µdeg coords for the nav graph.
/// Routability is tag-based ([`nav::is_routable`]) and independent of styling — a
/// way can be routable without a render style and vice-versa. Ways with fewer than
/// two nodes carry no edge and are skipped. `coords` is the way's f64-degree
/// geometry from [`resolve_coords`]; it is snapped to the µdeg grid here (the same
/// grid POIs and the serializer use) so edge lengths and later serialization agree.
fn push_routable_way(w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], out: &mut Vec<RoutableWay>) {
    if refs.len() < 2 {
        return;
    }
    // Classify once (routability + way-kind byte). `None` ⇒ not routable — this is
    // the only place tags exist, so the kind is captured here or never.
    let Some(kind) = nav::classify(w.tags()) else { return };
    let coords_udeg = coords.iter().map(|&(x, y)| (poi::to_udeg(x), poi::to_udeg(y))).collect();
    out.push(RoutableWay { node_ids: refs.to_vec(), coords: coords_udeg, kind });
}

/// Classify one node's tags against the POI table; push a candidate on match.
/// The overwhelmingly common untagged-node case falls straight through.
fn push_node_poi<'a, I>(tags: I, decimicro_lon: i32, decimicro_lat: i32, out: &mut Vec<Poi>)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    if let Some((subtype, name, raw_hours)) = poi::classify(tags) {
        out.push(Poi {
            subtype,
            lon_udeg: poi::to_udeg(to_deg(decimicro_lon)),
            lat_udeg: poi::to_udeg(to_deg(decimicro_lat)),
            name,
            from_node: true,
            hours: raw_hours.and_then(hours::parse),
        });
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
fn collect_relation(
    r: &osmpbf::Relation,
    config: &Config,
    pending: &mut Vec<PendingRelation>,
    needed_ways: &mut HashSet<i64>,
) {
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
    for &wid in &member_ways {
        needed_ways.insert(wid);
    }
    pending.push(PendingRelation { style_id: style.id, min_lod: style.min_lod, member_ways });
}

/// One way: capture coastline always, then style + classify into a single
/// polygon-or-line emission. `refs`/`coords` are pre-resolved.
fn process_way(
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    config: &Config,
    features: &mut Vec<IngestFeature>,
    coastlines: &mut Vec<Vec<(f64, f64)>>,
    pois: &mut Vec<Poi>,
) {
    let tags: HashMap<&str, &str> = w.tags().collect();
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled — and
    // as lines, never areas.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(coords.to_vec());
    }

    // A closed way matching the POI table yields a POI at the ring centroid —
    // independent of styling (a bare `shop=supermarket` outline has no style at
    // all). The building-tagged supermarket way and the area campsite are the
    // motivating cases; relations are out of scope (#115).
    if is_closed {
        if let Some((subtype, name, raw_hours)) = poi::classify(tags.iter().map(|(&k, &v)| (k, v))) {
            let (cx, cy) = poi::ring_centroid(coords);
            pois.push(Poi {
                subtype,
                lon_udeg: poi::to_udeg(cx),
                lat_udeg: poi::to_udeg(cy),
                name,
                from_node: false,
                hours: raw_hours.and_then(hours::parse),
            });
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
            features.push(IngestFeature {
                style_id: style.id,
                min_lod: style.min_lod,
                geom: Geom::Polygon { exterior: coords.to_vec(), interiors: Vec::new() },
            });
        }
        return;
    }

    // Line: open ways, and closed-but-not-area circular roads.
    if coords.len() >= 2 {
        features.push(IngestFeature { style_id: style.id, min_lod: style.min_lod, geom: Geom::Line(coords.to_vec()) });
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

    const TINY_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/tests/corpus/data/tiny.osm.pbf");

    fn is_polygon(g: &Geom) -> bool {
        matches!(g, Geom::Polygon { .. })
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
             packer/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let ing = ingest_osm(TINY_PBF, &cfg, None).expect("ingest");

        // W8 (way 109) is the only coastline; nodes 29,30 ⇒ 2 points.
        assert_eq!(ing.coastlines.len(), 1, "exactly one coastline");
        assert_eq!(ing.coastlines[0].len(), 2);

        // Multiset of (style_id, is_polygon).
        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        // Style ids: forest=39, pedestrian=15, residential=12, primary=5,
        // trunk=3, admin_level/2=42, water=32 (see config doc order).
        assert_eq!(n(39, true), 3, "W5 closed forest + R2's two outer rings ⇒ 3 polygons");
        assert_eq!(n(32, true), 1, "R1 natural=water ⇒ 1 polygon (lake)");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(42, false), 1, "W9 admin_level=2 ⇒ 1 line");
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
    /// unnamed). See packer/tests/corpus/poi/poi.osm.
    #[test]
    fn poi_fixture_end_to_end() {
        const POI_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/tests/corpus/data/poi.osm.pbf");
        assert!(
            std::path::Path::new(POI_PBF).exists(),
            "corpus fixture missing: {POI_PBF}. It is committed; rebuild from poi/poi.osm via \
             packer/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let ing = ingest_osm(POI_PBF, &cfg, None).expect("ingest");

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
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        // lon 7.798..7.809, lat 47.979..47.995 — see tiny.osm's node grid.
        let bbox = Bbox::parse("7.798,47.979,7.809,47.995").expect("box");
        let ing = ingest_osm(TINY_PBF, &cfg, Some(bbox)).expect("ingest");

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
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let plain = ingest_osm(TINY_PBF, &cfg, None).expect("ingest");
        let boxed = ingest_osm(TINY_PBF, &cfg, Some(Bbox::parse("-180,-90,180,90").expect("world"))).expect("ingest");
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
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let Err(err) = ingest_osm(TINY_PBF, &cfg, Some(Bbox::parse("10,10,11,11").expect("box"))) else {
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
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/tests/corpus/data/unsorted.osm.pbf");
        assert!(
            std::path::Path::new(UNSORTED_PBF).exists(),
            "corpus fixture missing: {UNSORTED_PBF}. It is committed; rebuild from unsorted/unsorted.osm via \
             packer/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        // The box covers both nodes, so a sorted file would have kept the way.
        let bbox = Bbox::parse("7.79,47.98,7.81,48.0").expect("box");
        let Err(err) = ingest_osm(UNSORTED_PBF, &cfg, Some(bbox)) else {
            panic!("an unsorted .pbf must not be cropped silently");
        };
        assert!(err.contains("not sorted"), "unexpected message: {err}");
        // Without a box the ingest is order-agnostic (passes 1 and 2 are separate
        // reads), so the same file still packs — the refusal is scoped to --bbox.
        let ing = ingest_osm(UNSORTED_PBF, &cfg, None).expect("uncropped ingest is order-agnostic");
        assert_eq!(ing.features.len(), 1, "the primary way survives without a box");
    }

    /// [`IdSet`] is only correct if `freeze` runs between filling and querying —
    /// and `freeze` must be safe to call twice (pass 0 freezes the node set early).
    #[test]
    fn id_set_freezes_and_dedupes() {
        let mut s = IdSet::default();
        for id in [9_i64, 3, 9, -1, 3] {
            s.insert(id);
        }
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
