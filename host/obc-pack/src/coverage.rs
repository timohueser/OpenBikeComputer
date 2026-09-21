//! Treats a tier's plain fills as one polygonal coverage: node every boundary into one planar
//! arrangement, give each face to the fill the device paint order makes visible there, dissolve the
//! faces per class, then hand every class to `GEOSCoverageSimplifyVW` in one call. The single call
//! is the point: an edge two classes share is simplified once, so both sides keep the identical
//! vertex sequence and cannot walk apart into a visible tear.
//!
//! A participating fill is [`crate::merge::merge_fills`]' notion of a plain fill: a polygon whose
//! style carries no `color2`. A face no fill covers is left as backdrop; the pass never invents
//! fill.
//!
//! On a coverage tier `min_area_px` is an elimination threshold, not a drop threshold: a face under
//! it joins the neighbour it shares the longest boundary with. Dropping it would punch a hole in a
//! tiling. The caller must not cull or simplify again what this pass marks as simplified.
//!
//! Any GEOS failure falls that component back to the ordinary per-feature path, so a bake never
//! drops map content.
use std::collections::{BTreeMap, HashMap};

use geos::{Geom as _, Geometry, PreparedGeometry, STRtree, SpatialIndex};
use rayon::prelude::*;

use obc_map_scene::M_PER_DEG;

use crate::geom::{
    box_polygon, collect_polygons, coverage_is_valid, coverage_simplify_vw, footprint_area_px, from_geos,
    try_polygon_to_geos, union_polygons, Bounds, Geom,
};
use crate::merge::ClassKey;
use crate::progress::Progress;

/// What the pass did to one LOD, for the per-tier log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CoverageStats {
    /// Participating fill polygons consumed.
    pub inputs: usize,
    /// Polygons the per-class pre-dissolve left — what the arrangement is built over.
    pub dissolved: usize,
    pub outputs: usize,
    pub vertices_in: usize,
    /// Ring positions that actually entered the arrangement, after the decimation pre-pass.
    pub vertices_arranged: usize,
    /// Bbox-connected components the arrangement was built over.
    pub components: usize,
    pub faces: usize,
    /// Faces no fill covered and no covered neighbour absorbed: genuine gaps, left as backdrop.
    pub dropped_faces: usize,
    /// Faces below the tier's threshold that were absorbed into a neighbour.
    pub eliminated: usize,
    /// Covered faces below the tier's threshold with no covered neighbour, so the ordinary
    /// footprint cull took them instead.
    pub uneliminable_culled: usize,
    /// Uncovered faces below the tier's threshold that a covered neighbour absorbed.
    pub healed: usize,
    /// Class groups whose `GEOSCoverageUnion` refused, leaving that class's faces undissolved.
    pub dissolve_failures: usize,
    /// Components that hit a GEOS failure and fell back to the per-feature path.
    pub fallbacks: usize,
}

/// GEOS `STRtree` node capacity: the number of children a node may hold, and not a count of items
/// to reserve room for. Pass an item count and the tree is one flat node, so every query scans
/// every envelope in it. 10 is the GEOS default.
const STRTREE_NODE_CAPACITY: usize = 10;

/// A member with more coordinates than this gets a `PreparedGeometry` for the face assignment;
/// smaller ones are point-tested directly. See [`assign_faces`].
const PREPARE_ABOVE_COORDS: usize = 64;

/// How many decimation tolerances of mean half-width an uncovered face may have and still be
/// healed into a covered neighbour (see [`sliver_half_width_m`]).
///
/// Two neighbours decimate their copies of a shared boundary independently, so a gap up to
/// `2 x dec_tol` wide can open, and a ribbon of width `w` has mean half-width `w/2`. A compact
/// shape fails this test long before its area would have saved it.
const HEAL_WIDTH_TOLERANCES: f64 = 2.0;

/// The decimation pre-pass runs at the tier's tolerance divided by this: deeply sub-pixel at the
/// scale the tier is drawn at, and an order of magnitude fewer vertices into the arrangement.
const DECIMATE_DIVISOR: f64 = 8.0;

/// Floor on the decimation tolerance, metres. A tier whose own tolerance is finer decimates at its
/// own tolerance instead: the pre-pass is never coarser than the pass it feeds.
const DECIMATE_FLOOR_M: f64 = 10.0;

/// The decimation tolerance for a tier simplifying at `tol` degrees, `0.0` for "do not decimate".
fn decimation_tol(tol: f64) -> f64 {
    if tol <= 0.0 {
        return 0.0;
    }
    (tol / DECIMATE_DIVISOR).max(DECIMATE_FLOOR_M / M_PER_DEG).min(tol)
}

/// The tier's small-face elimination threshold: `min_area_px` square pixels at `mpp`, the pair
/// [`crate::geom::footprint_below`] culls with, applied as an absorb-into-a-neighbour operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eliminate {
    /// The scale the threshold is measured at (the next-finer tier's `max_mpp`).
    pub mpp: f64,
    /// Faces below this many square pixels are absorbed.
    pub min_area_px: f64,
}

impl Eliminate {
    /// The pair, if both halves are usable. `None` disables elimination entirely.
    pub fn new(mpp: Option<f64>, min_area_px: f64) -> Option<Self> {
        let mpp = mpp?;
        (mpp > 0.0 && min_area_px > 0.0).then_some(Eliminate { mpp, min_area_px })
    }
}

/// A participating fill: one polygon of one plain-fill feature.
struct Fill {
    /// The feature's position in the tier's input order — the paint-order tiebreak.
    seq: usize,
    /// The style the polygon arrived with.
    style_id: u8,
    /// The class's canonical (smallest) style id — what a coverage result is tagged with.
    canonical: u8,
    /// `(z_index, color, priority)`; `z_index` is the paint-order key.
    key: ClassKey,
    geom: Geom,
    bounds: Bounds,
}

/// What the decimation pre-pass decided about one fill, and the geometry the arrangement uses.
enum Prep {
    /// GEOS will not touch it: it sits the arrangement out and passes through untouched.
    SitOut,
    /// Usable as it arrived: the tier asked for no simplify, or the decimation gave nothing usable.
    AsIs,
    /// Usable, decimated to [`decimation_tol`].
    Decimated(Geom),
}

/// A face's mean half-width on the ground, in metres: its area divided by its perimeter.
///
/// The ratio separates thin from small, which an area test cannot. A ribbon of width `w` answers
/// `w/2` however long it runs, a disc of radius `r` answers `r/2`, a square of side `s` answers
/// `s/4`. Longitude is foreshortened at the face's own mean latitude. A non-polygon or a degenerate
/// ring answers infinity, so it is never healed.
fn sliver_half_width_m(g: &Geom) -> f64 {
    let Geom::Polygon { exterior, interiors } = g else { return f64::INFINITY };
    let rings = || std::iter::once(exterior).chain(interiors.iter());
    let (mut lat_sum, mut n) = (0.0f64, 0usize);
    for r in rings() {
        for &(_, y) in r {
            lat_sum += y;
            n += 1;
        }
    }
    if n == 0 {
        return f64::INFINITY;
    }
    let cos_lat = (lat_sum / n as f64).to_radians().cos().abs().max(0.01);
    let (mut area, mut perimeter) = (0.0f64, 0.0f64);
    for (sign, ring) in std::iter::once((1.0, exterior)).chain(interiors.iter().map(|h| (-1.0, h))) {
        let mut shoelace = 0.0f64;
        for i in 0..ring.len() {
            let (ax, ay) = ring[i];
            let (bx, by) = ring[(i + 1) % ring.len()];
            let (ax, bx) = (ax * cos_lat, bx * cos_lat);
            shoelace += ax * by - bx * ay;
            perimeter += ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        }
        area += sign * (shoelace * 0.5).abs();
    }
    if perimeter <= 0.0 || area <= 0.0 {
        return f64::INFINITY;
    }
    area / perimeter * M_PER_DEG
}

fn vertex_count(g: &Geom) -> usize {
    match g {
        Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
        Geom::Line(c) => c.len(),
        Geom::Multi(parts) => parts.iter().map(vertex_count).sum(),
        Geom::Empty => 0,
    }
}

/// The identity of a tier's participating fill set, so [`PredissolveCache`] can tell whether the
/// dissolve it holds came from the same thing. `composition` is every fill's `(seq, style_id)` in
/// order; `geometry` hashes every coordinate, which is the one thing the composition cannot see. A
/// miss only costs the work again, so this errs towards missing.
#[derive(PartialEq, Eq)]
struct FillSetId {
    composition: Vec<(u32, u8)>,
    geometry: u64,
}

impl FillSetId {
    fn of(fills: &[Fill]) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut composition = Vec::with_capacity(fills.len());
        for f in fills {
            composition.push((f.seq as u32, f.style_id));
            // The kind discriminant enters the hash before the coordinates, so a non-polygon
            // cannot contribute nothing and let two different fill sets share a key.
            let kind: u8 = match &f.geom {
                Geom::Polygon { .. } => 1,
                Geom::Line(_) => 2,
                Geom::Multi(_) => 3,
                Geom::Empty => 4,
            };
            kind.hash(&mut h);
            let Geom::Polygon { exterior, interiors } = &f.geom else {
                debug_assert!(false, "a Fill is always a single polygon: split_geom flattens everything else");
                continue;
            };
            for ring in std::iter::once(exterior).chain(interiors.iter()) {
                ring.len().hash(&mut h);
                for &(x, y) in ring {
                    x.to_bits().hash(&mut h);
                    y.to_bits().hash(&mut h);
                }
            }
        }
        FillSetId { composition, geometry: h.finish() }
    }
}

/// A memo for [`predissolve`], shared by the coverage tiers of one build: every coverage tier
/// dissolves the same classes over the same fills, and only the decimation below it is per tier.
/// It holds an `Arc`, since the arrangement only reads the fills. [`PredissolveCache::clear`] drops
/// what is held, which the caller does once the last coverage tier is behind it.
#[derive(Default)]
pub struct PredissolveCache {
    entry: std::sync::Mutex<Option<(FillSetId, std::sync::Arc<Vec<Fill>>)>>,
}

impl PredissolveCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        if let Ok(mut slot) = self.entry.lock() {
            *slot = None;
        }
    }

    /// The dissolve of `fills` — from the memo if it was computed from exactly this set, otherwise
    /// computed now and memoised.
    ///
    /// The lock is never held across the dissolve, which is a minutes-long rayon fan-out: a mutex
    /// around a work-stealing pool deadlocks it. Two concurrent misses both compute, which is
    /// duplicated work and never a wrong answer.
    fn dissolve(&self, fills: Vec<Fill>, progress: &Progress) -> std::sync::Arc<Vec<Fill>> {
        let id = FillSetId::of(&fills);
        if let Ok(slot) = self.entry.lock() {
            if let Some((cached, dissolved)) = slot.as_ref() {
                if *cached == id {
                    return std::sync::Arc::clone(dissolved);
                }
            }
        }
        // Drop what is held before building the replacement, so the two never coexist.
        self.clear();
        let dissolved = std::sync::Arc::new(predissolve(fills, progress));
        if let Ok(mut slot) = self.entry.lock() {
            *slot = Some((id, std::sync::Arc::clone(&dissolved)));
        }
        dissolved
    }
}

/// Dissolve each class's fills into their union before the arrangement is built. A boundary
/// between two same-class parcels is invisible in the output, but the arrangement would still node
/// it, polygonize a face on each side, index both and dissolve them back together.
///
/// A dissolved polygon carries the whole class's first `seq`, because the pass emits a class as one
/// block of records at that position. A cancelled or refused class keeps its fills exactly as they
/// arrived: this is a cost optimisation, never a correctness step.
fn predissolve(fills: Vec<Fill>, progress: &Progress) -> Vec<Fill> {
    // Classes in canonical-id order; members in input order within each.
    let mut groups: BTreeMap<u8, Vec<Fill>> = BTreeMap::new();
    for f in fills {
        groups.entry(f.canonical).or_default().push(f);
    }
    let mut out: Vec<Fill> = Vec::new();
    for (canonical, members) in groups {
        if members.len() < 2 || progress.is_cancelled() {
            out.extend(members);
            continue;
        }
        let (key, seq) = (members[0].key, members.iter().map(|f| f.seq).min().expect("non-empty"));
        let refs: Vec<&Geom> = members.iter().map(|f| &f.geom).collect();
        match union_polygons(&refs) {
            Some(parts) => {
                drop(refs);
                for geom in parts {
                    if geom.is_empty() {
                        continue;
                    }
                    let bounds = geom.bounds();
                    out.push(Fill { seq, style_id: canonical, canonical, key, geom, bounds });
                }
            }
            None => {
                drop(refs);
                out.extend(members);
            }
        }
    }
    out
}

/// The decimation pre-pass: validate every fill and pre-simplify it at `dec_tol`. Each parallel
/// task builds, simplifies and reads back its own GEOS geometry, so nothing `!Send` crosses a
/// thread.
///
/// Anything GEOS refuses degrades rather than fails: an invalid input sits the arrangement out, and
/// a decimation that errors or comes back invalid leaves the fill at full detail. The arrangement
/// sees the same set of fills either way, only lighter.
fn prepare_fills(fills: &[Fill], dec_tol: f64, progress: &Progress) -> Vec<Prep> {
    fills
        .par_iter()
        .map(|f| {
            if progress.is_cancelled() {
                return Prep::AsIs;
            }
            let Some(g) = try_polygon_to_geos(&f.geom) else { return Prep::SitOut };
            if !g.is_valid().unwrap_or(false) {
                return Prep::SitOut;
            }
            if dec_tol <= 0.0 {
                return Prep::AsIs;
            }
            let Ok(s) = g.topology_preserve_simplify(dec_tol) else { return Prep::AsIs };
            // A simplify that broke validity is not an input this pass may node.
            if !s.is_valid().unwrap_or(false) {
                return Prep::AsIs;
            }
            match from_geos(&s) {
                g @ Geom::Polygon { .. } if !g.is_empty() => Prep::Decimated(g),
                _ => Prep::AsIs,
            }
        })
        .collect()
}

/// One emission slot in input order: a passthrough feature, or a class's coverage output emitted
/// at its first member's position.
enum Slot {
    Pass(u8, Geom),
    Group(u8),
}

impl Slot {
    /// The emitted feature of a passthrough slot. A `Group` slot only exists where a fill joined
    /// it, so it never reaches here.
    fn into_pass(self) -> (u8, Geom, bool) {
        match self {
            Slot::Pass(sid, g) => (sid, g, false),
            Slot::Group(sid) => (sid, Geom::Empty, false),
        }
    }
}

/// Coverage-simplify a tier's plain fills. `classes` is the [`crate::merge::merge_classes`] table
/// (a style is a plain fill iff it is in there) and `tol` the tier's simplify tolerance in degrees
/// (`0.0` means dissolve and re-cut, but do not simplify).
///
/// Returns `(style_id, geom, simplified)` in slot order. `simplified == true` means the pass
/// already applied the tier's tolerance and cull, and the caller must not apply either again.
pub fn coverage_simplify_fills(
    features: Vec<(u8, Geom)>,
    classes: &HashMap<u8, (ClassKey, u8)>,
    tol: f64,
    eliminate: Option<Eliminate>,
) -> (Vec<(u8, Geom, bool)>, CoverageStats) {
    coverage_simplify_fills_with(features, classes, tol, eliminate, &PredissolveCache::new(), &Progress::silent())
}

/// [`coverage_simplify_fills`], abandonable. The checkpoint is per component: an arrangement over a
/// big cluster runs for seconds inside GEOS and cannot be interrupted from outside. A cancelled
/// component takes the fallback path, so the output stays well-formed.
pub fn coverage_simplify_fills_with(
    features: Vec<(u8, Geom)>,
    classes: &HashMap<u8, (ClassKey, u8)>,
    tol: f64,
    eliminate: Option<Eliminate>,
    cache: &PredissolveCache,
    progress: &Progress,
) -> (Vec<(u8, Geom, bool)>, CoverageStats) {
    let mut slots: Vec<Slot> = Vec::with_capacity(features.len());
    let mut fills: Vec<Fill> = Vec::new();
    let mut seen_class: Vec<u8> = Vec::new();
    for (seq, (style_id, geom)) in features.into_iter().enumerate() {
        let Some(&(key, canonical)) = classes.get(&style_id) else {
            slots.push(Slot::Pass(style_id, geom));
            continue;
        };
        let mut polys = Vec::new();
        let mut others = Vec::new();
        split_geom(geom, &mut polys, &mut others);
        for o in others {
            slots.push(Slot::Pass(style_id, o));
        }
        for p in polys {
            if p.is_empty() {
                continue;
            }
            let bounds = p.bounds();
            if !seen_class.contains(&canonical) {
                seen_class.push(canonical);
                slots.push(Slot::Group(canonical));
            }
            fills.push(Fill { seq, style_id, canonical, key, geom: p, bounds });
        }
    }

    let mut stats = CoverageStats { inputs: fills.len(), ..Default::default() };
    stats.vertices_in = fills.iter().map(|f| vertex_count(&f.geom)).sum();
    if fills.is_empty() {
        return (slots.into_iter().map(|s| s.into_pass()).collect(), stats);
    }

    // Decimation only shrinks a polygon's bounds, so components computed from the post-dissolve
    // bounds stay the conservative superset they have to be. It is gated on the tier having an
    // elimination threshold, because that threshold is what closes the micro-gaps that decimating
    // each fill independently opens.
    let fills = cache.dissolve(fills, progress);
    stats.dissolved = fills.len();
    let dec_tol = if eliminate.is_some() { decimation_tol(tol) } else { 0.0 };
    // The sliver bound healing measures uncovered faces against, in metres. A tier that does not
    // decimate opens no gaps and heals none.
    let heal_half_width_m = HEAL_WIDTH_TOLERANCES * dec_tol * M_PER_DEG;
    let preps = prepare_fills(&fills, dec_tol, progress);
    stats.vertices_arranged = fills
        .iter()
        .zip(&preps)
        .map(|(f, p)| match p {
            Prep::SitOut => 0,
            Prep::AsIs => vertex_count(&f.geom),
            Prep::Decimated(g) => vertex_count(g),
        })
        .sum();
    let components = bbox_components(&fills);
    stats.components = components.len();

    // One arrangement per component, in parallel. Every GEOS object a task touches is built, used
    // and dropped on that task's own thread (`geos::Geometry` is `!Send`); only plain `Geom`
    // crosses a thread boundary.
    let results: Vec<Option<ComponentOut>> = components
        .par_iter()
        .map(|comp| {
            if progress.is_cancelled() {
                None
            } else {
                coverage_component(&fills, &preps, comp, tol, eliminate, heal_half_width_m)
            }
        })
        .collect();

    // Emit in slot order, each class's polygons at its first member's position.
    let mut by_class: HashMap<u8, Vec<(u8, Geom, bool)>> = HashMap::new();
    for (comp, result) in components.iter().zip(results) {
        match result {
            Some(out) => {
                stats.faces += out.faces;
                stats.dropped_faces += out.dropped_faces;
                stats.eliminated += out.eliminated;
                stats.uneliminable_culled += out.uneliminable_culled;
                stats.healed += out.healed;
                stats.dissolve_failures += out.dissolve_failures;
                stats.outputs += out.polys.len();
                for (style_id, g, simplified) in out.polys {
                    let canonical = classes.get(&style_id).map(|&(_, c)| c).unwrap_or(style_id);
                    by_class.entry(canonical).or_default().push((style_id, g, simplified));
                }
            }
            // GEOS said no: hand this component's fills back untouched, original style ids,
            // marked for the ordinary per-feature simplify.
            None => {
                stats.fallbacks += 1;
                for &i in comp {
                    let f = &fills[i];
                    stats.outputs += 1;
                    by_class.entry(f.canonical).or_default().push((f.style_id, f.geom.clone(), false));
                }
            }
        }
    }
    let mut out: Vec<(u8, Geom, bool)> = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Slot::Pass(sid, g) => out.push((sid, g, false)),
            Slot::Group(canonical) => {
                if let Some(group) = by_class.remove(&canonical) {
                    out.extend(group);
                }
            }
        }
    }
    (out, stats)
}

/// Split a geometry into its polygon parts (coverage candidates) and everything else (lines pass
/// through): flatten `Multi`, drop `Empty`.
fn split_geom(g: Geom, polys: &mut Vec<Geom>, others: &mut Vec<Geom>) {
    match g {
        p @ Geom::Polygon { .. } => polys.push(p),
        Geom::Multi(parts) => {
            for p in parts {
                split_geom(p, polys, others);
            }
        }
        Geom::Empty => {}
        line => others.push(line),
    }
}

/// What one component's arrangement produced.
struct ComponentOut {
    /// The coverage output in class order then GEOS order, followed by the members that sat the
    /// arrangement out. Deterministic.
    polys: Vec<(u8, Geom, bool)>,
    faces: usize,
    dropped_faces: usize,
    eliminated: usize,
    uneliminable_culled: usize,
    healed: usize,
    dissolve_failures: usize,
}

/// Partition fills into connected components under bounding-box intersection.
///
/// Two polygons that share an edge or overlap necessarily have intersecting boxes, so every
/// interaction stays inside one component and two components are provably disjoint. A union-find
/// decides membership and the groups are read off by walking `0..n`, so the result cannot depend on
/// query order. A tree that will not build degenerates to one component: correct, merely slower.
fn bbox_components(fills: &[Fill]) -> Vec<Vec<usize>> {
    let n = fills.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path halving
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    // A box that will not build links to nothing, so that polygon forms its own component and is
    // coverage-simplified alone.
    let boxes: Vec<Option<Geometry>> = fills.iter().map(|f| box_polygon(f.bounds).ok()).collect();
    if let Ok(mut tree) = STRtree::<usize>::with_capacity(STRTREE_NODE_CAPACITY) {
        for (i, b) in boxes.iter().enumerate() {
            if let Some(b) = b {
                tree.insert(b, i);
            }
        }
        for (i, b) in boxes.iter().enumerate() {
            let Some(b) = b else { continue };
            let mut hits: Vec<usize> = Vec::new();
            tree.query(b, |&j: &usize| hits.push(j));
            for j in hits {
                union(&mut parent, i, j);
            }
        }
    } else {
        for i in 1..n {
            union(&mut parent, 0, i);
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots_in_order: Vec<usize> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        let g = groups.entry(r).or_default();
        if g.is_empty() {
            roots_in_order.push(r);
        }
        g.push(i);
    }
    roots_in_order.into_iter().map(|r| groups.remove(&r).expect("a listed root has a group")).collect()
}

/// The whole pass for one component: arrangement, face assignment, per-class dissolve, one coverage
/// simplify. `None` on any GEOS failure, which the caller turns into a pass-through of this
/// component's fills.
///
/// Invalid members sit it out. GEOS answers point-in-polygon questions about a self-intersecting
/// ring however it likes, and a face nobody is found to cover is dropped, so one broken parcel
/// could silently delete itself. `preps` is the pre-pass verdict per fill: it decides which members
/// sit out and supplies the lighter geometry the arrangement is built from.
fn coverage_component(
    fills: &[Fill],
    preps: &[Prep],
    comp: &[usize],
    tol: f64,
    eliminate: Option<Eliminate>,
    heal_half_width_m: f64,
) -> Option<ComponentOut> {
    let mut members: Vec<Geometry> = Vec::with_capacity(comp.len());
    let mut member_of: Vec<usize> = Vec::with_capacity(comp.len());
    // The `Geom` each member was built from, for the single-member shortcut below.
    let mut prepared: Vec<&Geom> = Vec::with_capacity(comp.len());
    let mut sat_out: Vec<(u8, Geom, bool)> = Vec::new();
    for (k, &i) in comp.iter().enumerate() {
        let geom: &Geom = match &preps[i] {
            Prep::SitOut => {
                sat_out.push((fills[i].style_id, fills[i].geom.clone(), false));
                continue;
            }
            Prep::AsIs => &fills[i].geom,
            Prep::Decimated(g) => g,
        };
        match try_polygon_to_geos(geom) {
            Some(g) => {
                members.push(g);
                member_of.push(k);
                prepared.push(geom);
            }
            None => sat_out.push((fills[i].style_id, fills[i].geom.clone(), false)),
        }
    }
    if members.is_empty() {
        return None;
    }

    // A lone fill is its own arrangement, which skips the overlay machinery for the common
    // isolated polygon. `members` is scoped to this block because on a wall-to-wall landuse cluster
    // it is tens of thousands of live GEOS polygons that the dissolve and simplify below have no
    // use for.
    let (faces, mut winners) = {
        let members = members; // moved in, so the block's end frees them
        if members.len() == 1 {
            (vec![prepared[0].clone()], vec![Some(0usize)])
        } else {
            let faces = arrangement_faces(&members)?;
            let winners = assign_faces(&faces, &members, fills, comp, &member_of)?;
            (faces, winners)
        }
    };
    let n_faces = faces.len();

    // --- Elimination: a face under the tier's threshold joins the neighbour it shares the most
    let (eliminated, uneliminable_culled, healed) = match eliminate {
        Some(e) => eliminate_small_faces(&faces, &mut winners, e, heal_half_width_m),
        None => (0, 0, 0),
    };
    // After elimination, because healing is what turns an uncovered face into a covered one: what
    // is still uncovered here stays backdrop. Culled faces are counted separately from gaps.
    let dropped = winners.iter().filter(|w| w.is_none()).count() - uneliminable_culled;

    // Per-class dissolve, classes in key order so emission is deterministic.
    let mut by_class: BTreeMap<ClassKey, (u8, Vec<Geom>)> = BTreeMap::new();
    for (face, winner) in faces.into_iter().zip(&winners) {
        let Some(w) = winner else { continue };
        let f = &fills[comp[member_of[*w]]];
        by_class.entry(f.key).or_insert_with(|| (f.canonical, Vec::new())).1.push(face);
    }
    let mut elements: Vec<Geom> = Vec::new();
    let mut owner: Vec<u8> = Vec::new();
    let mut dissolve_failures = 0;
    for (_key, (canonical, group)) in by_class {
        let dissolved = match dissolve_class(&group) {
            Some(d) => d,
            None => {
                if group.len() > 1 {
                    dissolve_failures += 1;
                }
                group
            }
        };
        for g in dissolved {
            if g.is_empty() {
                continue;
            }
            elements.push(g);
            owner.push(canonical);
        }
    }
    if elements.is_empty() {
        return Some(ComponentOut {
            polys: sat_out,
            faces: n_faces,
            dropped_faces: dropped,
            eliminated,
            uneliminable_culled,
            healed,
            dissolve_failures,
        });
    }

    // One coverage simplify over every class at once, so an edge shared by two classes is
    // simplified once and both sides keep the identical vertex sequence.
    let mut polys: Vec<(u8, Geom, bool)> = Vec::new();
    if tol > 0.0 {
        let refs: Vec<&Geom> = elements.iter().collect();
        // The simplifier assumes a valid coverage; on anything else its output is not glued, so
        // refuse rather than ship a subtly torn tier.
        if !coverage_is_valid(&refs, 0.0) {
            return None;
        }
        let simplified = coverage_simplify_vw(&refs, tol, false)?;
        for (g, canonical) in simplified.into_iter().zip(&owner) {
            let mut parts = Vec::new();
            collect_polygons(g, &mut parts);
            for p in parts {
                polys.push((*canonical, p, true));
            }
        }
    } else {
        for (g, canonical) in elements.into_iter().zip(&owner) {
            let mut parts = Vec::new();
            collect_polygons(g, &mut parts);
            for p in parts {
                polys.push((*canonical, p, true));
            }
        }
    }
    polys.extend(sat_out);
    Some(ComponentOut {
        polys,
        faces: n_faces,
        dropped_faces: dropped,
        eliminated,
        uneliminable_culled,
        healed,
        dissolve_failures,
    })
}

/// Node every member's boundary into one planar arrangement and polygonize it into faces. Noding is
/// what makes the edges shared: two fills that abut then carry one edge between them instead of two
/// copies, and every later step is exact rather than approximate.
fn arrangement_faces(members: &[Geometry]) -> Option<Vec<Geom>> {
    let mut lines: Vec<Geometry> = Vec::with_capacity(members.len());
    for m in members {
        lines.push(m.boundary().ok()?);
    }
    // Each step copies every coordinate again, so each input is freed the moment its successor
    // exists: on a country-scale arrangement a spare copy of every boundary is hundreds of
    // megabytes.
    let noded = {
        let collection = Geometry::create_geometry_collection(lines).ok()?;
        collection.node().ok()?
    };
    let polygonized = Geometry::polygonize(&[noded]).ok()?;
    let mut faces = Vec::new();
    collect_polygons(from_geos(&polygonized), &mut faces);
    (!faces.is_empty()).then_some(faces)
}

/// For each face, the index (into `members`) of the fill that would be visible there, or `None` for
/// a face nothing covers. `member_of[i]` maps that back to a position in `comp`.
///
/// The representative point is a `GEOSPointOnSurface`, which lies in the face interior, so coverage
/// is one point test per candidate, with an `STRtree` to keep the candidate set small. The winner
/// is the maximum by `(z_index, seq)`, the device paint order, and a later span paints over an
/// earlier one.
///
/// Only members over [`PREPARE_ABOVE_COORDS`] are prepared. A `PreparedGeometry` earns its index
/// when the same shape is queried many times; for a handful of vertices tested two or three times
/// it costs far more than the ray casts it saves.
fn assign_faces(
    faces: &[Geom],
    members: &[Geometry],
    fills: &[Fill],
    comp: &[usize],
    member_of: &[usize],
) -> Option<Vec<Option<usize>>> {
    let prepared: Vec<Option<PreparedGeometry<'_>>> = members
        .iter()
        .map(|m| {
            let big = m.get_num_coordinates().unwrap_or(0) > PREPARE_ABOVE_COORDS;
            big.then(|| m.to_prepared_geom()).transpose()
        })
        .collect::<Result<_, _>>()
        .ok()?;
    let mut tree = STRtree::<usize>::with_capacity(STRTREE_NODE_CAPACITY).ok()?;
    for (i, m) in members.iter().enumerate() {
        tree.insert(m, i);
    }
    let mut out = Vec::with_capacity(faces.len());
    for face in faces {
        let geos_face = try_polygon_to_geos(face)?;
        let point = geos_face.point_on_surface().ok()?;
        let (x, y) = (point.get_x().ok()?, point.get_y().ok()?);
        let mut hits: Vec<usize> = Vec::new();
        tree.query(&point, |&i: &usize| hits.push(i));
        hits.sort_unstable();
        let mut best: Option<usize> = None;
        for i in hits {
            // A point test GEOS could not answer is a failure, not a "no": treating it as a miss
            // would drop the face and with it a piece of the map.
            let inside = match &prepared[i] {
                Some(p) => p.contains_xy(x, y).ok()?,
                None => members[i].contains(&point).ok()?,
            };
            if !inside {
                continue;
            }
            let rank = |k: usize| {
                let f = &fills[comp[member_of[k]]];
                (f.key.0, f.seq)
            };
            if best.is_none_or(|b| rank(i) > rank(b)) {
                best = Some(i);
            }
        }
        out.push(best);
    }
    Some(out)
}

/// Absorb faces under the tier's threshold into their neighbours until nothing under it is left,
/// returning `(covered faces that changed class, covered faces the cull took, uncovered faces a
/// neighbour healed)`.
///
/// The smallest cluster still under the threshold joins the neighbouring cluster it shares the
/// longest boundary with and takes its class. Absorbing grows the survivor, so the pass runs to a
/// fixed point: one sweep would leave every speck settled on the speck next door and the threshold
/// binding nothing. Adjacency is read straight off the arrangement, because polygonize emits faces
/// that share their edges vertex for vertex. Merge order `(area, cluster id)` and target choice
/// `(shared length, cluster id)` are total orders, so the result cannot depend on hash or thread
/// order.
///
/// An uncovered face is absorbed into a covered neighbour only when its [`sliver_half_width_m`] is
/// under `heal_half_width_m`: that is the healing that closes micro-gaps. Never the reverse, since
/// every target is a covered cluster, so a covered face can never be swallowed by a gap.
/// `heal_half_width_m <= 0.0` turns healing off.
///
/// A covered cluster the fixed point leaves under the threshold is an island — everything touching
/// it is backdrop — so the footprint cull is applied here instead, at the tier's own
/// `(mpp, min_area_px)`, and counted separately. The caller skips its own cull for everything this
/// pass produced. The decision is made after the loop, so it cannot depend on the order faces were
/// popped in.
fn eliminate_small_faces(
    faces: &[Geom],
    winners: &mut [Option<usize>],
    e: Eliminate,
    heal_half_width_m: f64,
) -> (usize, usize, usize) {
    let n = faces.len();
    // Adjacency: undirected segment -> the face(s) carrying it. A sorted flat `Vec`, not a
    // `HashMap<Key, Vec<_>>`: a country-scale arrangement carries millions of segments, and the
    // buckets and per-key `Vec` headers cost more than the coordinates do. After the sort the two
    // faces sharing an edge are adjacent entries.
    let mut edges: Vec<(u64, u64, u64, u64, u32)> = Vec::new();
    {
        let bits = |(x, y): (f64, f64)| (x.to_bits(), y.to_bits());
        let push_ring = |ring: &[(f64, f64)], fi: u32, edges: &mut Vec<_>| {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if a == b {
                    continue;
                }
                let (ka, kb) = (bits(a), bits(b));
                let ((x0, y0), (x1, y1)) = if ka <= kb { (ka, kb) } else { (kb, ka) };
                edges.push((x0, y0, x1, y1, fi));
            }
        };
        for (fi, face) in faces.iter().enumerate() {
            if let Geom::Polygon { exterior, interiors } = face {
                push_ring(exterior, fi as u32, &mut edges);
                for hole in interiors {
                    push_ring(hole, fi as u32, &mut edges);
                }
            }
        }
        edges.sort_unstable();
    }

    // Shared ground length per unordered face pair, at least one side covered, or there is no
    // absorption either way. A run of one is an outer edge; a run of more than two is non-manifold
    // and the arrangement should not produce it, so both are skipped rather than guessed at.
    let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
    let mut k = 0;
    while k < edges.len() {
        let (x0, y0, x1, y1, _) = edges[k];
        let mut end = k + 1;
        while end < edges.len() && edges[end].0 == x0 && edges[end].1 == y0 && edges[end].2 == x1 && edges[end].3 == y1
        {
            end += 1;
        }
        let run = &edges[k..end];
        k = end;
        let owners: Vec<usize> = {
            let mut o: Vec<usize> = run.iter().map(|r| r.4 as usize).collect();
            o.dedup();
            o
        };
        if owners.len() != 2 {
            continue;
        }
        let (i, j) = (owners[0], owners[1]);
        if winners[i].is_none() && winners[j].is_none() {
            continue;
        }
        let (ax, ay, bx, by) = (f64::from_bits(x0), f64::from_bits(y0), f64::from_bits(x1), f64::from_bits(y1));
        let cos_lat = (0.5 * (ay + by)).to_radians().cos().abs().max(0.01);
        let (dx, dy) = ((bx - ax) * cos_lat * M_PER_DEG, (by - ay) * M_PER_DEG);
        let len = (dx * dx + dy * dy).sqrt();
        *adj[i].entry(j).or_insert(0.0) += len;
        *adj[j].entry(i).or_insert(0.0) += len;
    }
    drop(edges);

    // Clusters: a union-find whose representative carries the class and the running area.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path halving
            x = parent[x];
        }
        x
    }
    let mut area: Vec<f64> = faces.iter().map(|f| footprint_area_px(f, e.mpp)).collect();
    // A min-heap over (area, id) with lazy invalidation: a cluster is re-pushed whenever it grows,
    // and a pop whose area no longer matches the live one is stale. Uncovered faces are seeded only
    // when thin, so a healed face is always a subset of what a covered face of the same size would
    // be.
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(OrdF64, usize)>> = faces
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            area[*i] < e.min_area_px
                && (winners[*i].is_some() || (heal_half_width_m > 0.0 && sliver_half_width_m(f) < heal_half_width_m))
        })
        .map(|(i, _)| std::cmp::Reverse((OrdF64(area[i]), i)))
        .collect();

    while let Some(std::cmp::Reverse((OrdF64(a), small))) = heap.pop() {
        if find(&mut parent, small) != small || a != area[small] {
            continue; // stale: already absorbed, or re-pushed with a bigger area
        }
        if area[small] >= e.min_area_px {
            continue; // grew past the threshold while it waited
        }
        // The neighbour cluster sharing the most boundary; ties by lowest id so the choice is
        // stable. Only a covered cluster may be the target: absorbing into a gap would delete fill.
        let mut best: Option<(usize, f64)> = None;
        for (&other, &len) in &adj[small] {
            let root = find(&mut parent, other);
            if root == small || winners[root].is_none() {
                continue;
            }
            if best.is_none_or(|(bi, bl)| len > bl || (len == bl && root < bi)) {
                best = Some((root, len));
            }
        }
        let Some((into, _)) = best else {
            // An island with nothing covered beside it. The sweep after the loop decides its fate:
            // covered means the cull takes it, uncovered means it stays backdrop.
            continue;
        };
        // The survivor keeps its own id, so the heap's other entries for it stay meaningful.
        parent[small] = into;
        area[into] += area[small];
        let moved: Vec<(usize, f64)> = adj[small].drain().collect();
        for (other, len) in moved {
            if find(&mut parent, other) == into {
                continue; // the edge between the two is interior now
            }
            *adj[into].entry(other).or_insert(0.0) += len;
            // The neighbour's own entry has to follow, or it would keep pointing at a dead cluster.
            if let Some(l) = adj[other].remove(&small) {
                *adj[other].entry(into).or_insert(0.0) += l;
            }
        }
        if area[into] < e.min_area_px {
            heap.push(std::cmp::Reverse((OrdF64(area[into]), into)));
        }
    }

    // Every face takes its cluster's owner. A root is always covered by the time anything joins it,
    // so a face can only gain fill here, never lose it — except through the cull below. Which
    // clusters the cull takes is decided before anything is written back, because the test reads
    // the root's own `winners` entry and the loop is about to clear it.
    let mut cull_cluster: Vec<bool> = vec![false; n];
    for r in 0..n {
        if find(&mut parent, r) == r {
            cull_cluster[r] = winners[r].is_some() && area[r] < e.min_area_px;
        }
    }
    let (mut moved, mut culled, mut healed) = (0, 0, 0);
    for fi in 0..n {
        let root = find(&mut parent, fi);
        // The uneliminable case: a covered cluster still under the threshold after the fixed point.
        if cull_cluster[root] {
            if winners[fi].is_some() {
                winners[fi] = None;
                culled += 1;
            }
            continue;
        }
        if root == fi {
            continue;
        }
        let Some(owner) = winners[root] else { continue };
        match winners[fi] {
            None => {
                winners[fi] = Some(owner);
                healed += 1;
            }
            Some(w) if w != owner => {
                winners[fi] = Some(owner);
                moved += 1;
            }
            Some(_) => {}
        }
    }
    (moved, culled, healed)
}

/// A total order over the `f64` areas the heap sorts by: every value is a finite, non-negative
/// projected area, so `total_cmp` is an ordinary comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrdF64(f64);

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Dissolve one class's faces with `GEOSCoverageUnion`, the cheap union that assumes what is
/// already true here: the faces come out of one arrangement, so their shared edges match vertex for
/// vertex. `None` on a GEOS failure, and on a class of one, which leaves the faces as they are.
fn dissolve_class(faces: &[Geom]) -> Option<Vec<Geom>> {
    if faces.len() < 2 {
        return None;
    }
    let mut geoms = Vec::with_capacity(faces.len());
    for f in faces {
        geoms.push(try_polygon_to_geos(f)?);
    }
    let collection = Geometry::create_multipolygon(geoms).ok()?;
    let unioned = collection.coverage_union().ok()?;
    let mut out = Vec::new();
    collect_polygons(from_geos(&unioned), &mut out);
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LineStyle;
    use crate::merge::merge_classes;
    use crate::serialize::Style;

    /// A plain-fill style (no `color2`) — the only kind that participates.
    fn fill_style(id: u8, z_index: i8, color: u16) -> Style {
        Style {
            id,
            z_index,
            color,
            weight: 1,
            priority: 3,
            line_style: LineStyle::Solid,
            color2: None,
            fixed_width: false,
            terrain_layer: false,
        }
    }

    fn poly(ring: &[(f64, f64)]) -> Geom {
        let mut exterior = ring.to_vec();
        if exterior.first() != exterior.last() {
            exterior.push(exterior[0]);
        }
        Geom::Polygon { exterior, interiors: vec![] }
    }

    /// An axis-aligned box.
    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Geom {
        poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
    }

    fn area(features: &[(u8, Geom, bool)]) -> f64 {
        fn ring_area(r: &[(f64, f64)]) -> f64 {
            let mut a = 0.0;
            for i in 0..r.len() {
                let (x1, y1) = r[i];
                let (x2, y2) = r[(i + 1) % r.len()];
                a += x1 * y2 - x2 * y1;
            }
            (a * 0.5).abs()
        }
        let mut sum = 0.0;
        for (_, g, _) in features {
            if let Geom::Polygon { exterior, interiors } = g {
                sum += ring_area(exterior);
                for h in interiors {
                    sum -= ring_area(h);
                }
            }
        }
        sum
    }

    /// Every vertex of a feature's rings, sorted — a shape-independent fingerprint.
    fn verts(g: &Geom) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        fn walk(g: &Geom, out: &mut Vec<(f64, f64)>) {
            match g {
                Geom::Line(c) => out.extend(c.iter().copied()),
                Geom::Polygon { exterior, interiors } => {
                    out.extend(exterior.iter().copied());
                    for h in interiors {
                        out.extend(h.iter().copied());
                    }
                }
                Geom::Multi(parts) => parts.iter().for_each(|p| walk(p, out)),
                Geom::Empty => {}
            }
        }
        walk(g, &mut out);
        out.sort_by(|a, b| a.partial_cmp(b).expect("finite coords"));
        out.dedup();
        out
    }

    /// The shared seam both fills carry, from (1,0) up to (1,1). Two abutting OSM ways reference
    /// the same boundary nodes, so both copies are identical here too.
    const SEAM: [(f64, f64); 6] = [(1.12, 0.2), (0.95, 0.35), (1.18, 0.5), (0.9, 0.62), (1.14, 0.8), (1.0, 1.0)];

    /// The western fill: a tall slab whose right edge carries the seam and then runs straight up to
    /// y=10. Its ring is far longer than its neighbour's, so a per-feature Douglas-Peucker resolves
    /// the shared chain differently on the two sides.
    fn seam_west() -> Geom {
        let mut ring = vec![(0.0, 0.0), (1.0, 0.0)];
        ring.extend(SEAM.iter().copied());
        ring.extend([(1.0, 10.0), (0.0, 10.0)]);
        poly(&ring)
    }

    fn seam_east() -> Geom {
        let mut ring = vec![(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
        ring.extend(SEAM.iter().rev().skip(1).copied());
        poly(&ring)
    }

    /// A feature's vertices in the seam band — the two copies of the shared boundary.
    fn seam_verts(g: &Geom) -> Vec<(f64, f64)> {
        verts(g).into_iter().filter(|(x, y)| *x > 0.5 && *x < 1.5 && *y < 1.001).collect()
    }

    #[test]
    fn a_shared_boundary_is_identical_on_both_sides() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 0, 0x0002)]);
        let (out, stats) = coverage_simplify_fills(vec![(1, seam_west()), (2, seam_east())], &classes, 0.2, None);
        assert_eq!(stats.fallbacks, 0, "no GEOS failure: {stats:?}");
        assert_eq!(out.len(), 2, "one polygon per class: {out:?}");
        assert!(out.iter().all(|(_, _, simplified)| *simplified), "both carry the coverage tolerance");

        let a = seam_verts(&out[0].1);
        let b = seam_verts(&out[1].1);
        assert!(!a.is_empty(), "the seam did not vanish entirely");
        assert_eq!(a, b, "the shared boundary must be the SAME vertices on both sides");
        assert!(a.len() < SEAM.len() + 1, "and it really was simplified: {a:?}");
    }

    #[test]
    fn the_per_feature_path_tears_the_same_seam() {
        let a = crate::geom::topology_preserve_simplify(&seam_west(), 0.2);
        let b = crate::geom::topology_preserve_simplify(&seam_east(), 0.2);
        assert_ne!(
            seam_verts(&a),
            seam_verts(&b),
            "if this ever matches, the fixture stopped exercising the tear this pass fixes"
        );
    }

    #[test]
    fn an_overlap_goes_to_the_class_on_top() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let under = rect(0.0, 0.0, 2.0, 2.0); // 4 deg²
        let over = rect(1.0, 0.0, 3.0, 2.0); // 4 deg², overlapping the right half of `under`
        let (out, stats) = coverage_simplify_fills(vec![(1, under), (2, over)], &classes, 0.0, None);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!((area(&out) - 6.0).abs() < 1e-9, "the 2 deg² overlap is stored once, not twice: {out:?}");
        let by_style = |sid: u8| area(&out.iter().filter(|(s, _, _)| *s == sid).cloned().collect::<Vec<_>>());
        assert!((by_style(2) - 4.0).abs() < 1e-9, "the top class keeps its whole footprint");
        assert!((by_style(1) - 2.0).abs() < 1e-9, "the covered half of the bottom class is gone");
    }

    /// Eight unit squares of one class around an unmapped centre cell, plus a neighbour of a second
    /// class abutting the block's right edge. The neighbour is what makes the component a real
    /// arrangement rather than the single-member shortcut, so the hole reaches [`assign_faces`] as a
    /// face nothing covers.
    fn ring_around_a_hole() -> Vec<(u8, Geom)> {
        let mut feats = Vec::new();
        for gx in 0..3 {
            for gy in 0..3 {
                if gx == 1 && gy == 1 {
                    continue;
                }
                feats.push((1u8, rect(gx as f64, gy as f64, gx as f64 + 1.0, gy as f64 + 1.0)));
            }
        }
        feats.push((2u8, rect(3.0, 0.0, 4.0, 1.0)));
        feats
    }

    fn hole_count(out: &[(u8, Geom, bool)]) -> usize {
        out.iter()
            .map(|(_, g, _)| match g {
                Geom::Polygon { interiors, .. } => interiors.len(),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn an_uncovered_face_is_not_invented() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let (out, stats) = coverage_simplify_fills(ring_around_a_hole(), &classes, 0.0, None);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!(stats.dropped_faces >= 1, "the centre face belongs to nobody: {stats:?}");
        assert!((area(&out) - 9.0).abs() < 1e-9, "nine squares in, nine squares of area out");
        assert_eq!(hole_count(&out), 1, "the unmapped centre survives as a hole, not as fill");
    }

    /// A threshold in the units the pass takes: `px^2` at `mpp`. One square degree at the equator
    /// is `(M_PER_DEG / mpp)^2` pixels, so this converts an area in square degrees into the
    /// `min_area_px` that sits exactly on it.
    fn threshold_for(deg2: f64, mpp: f64) -> Eliminate {
        Eliminate { mpp, min_area_px: deg2 * (M_PER_DEG / mpp) * (M_PER_DEG / mpp) }
    }

    #[test]
    fn a_small_face_is_absorbed_by_its_longest_neighbour() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let host = rect(0.0, 0.0, 1.0, 1.0);
        let speck = rect(0.0, 0.4, 0.1, 0.5); // 0.01 deg², on top (z 5) so it wins its own face
        let mpp = 100.0;
        let e = Some(threshold_for(0.5, mpp));
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, speck)], &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.eliminated, 1, "exactly the speck moved: {stats:?}");
        assert!((area(&out) - 1.0).abs() < 1e-9, "the ground is conserved, not culled: {out:?}");
        assert!(out.iter().all(|(sid, _, _)| *sid == 1), "and all of it belongs to the host class: {out:?}");
    }

    #[test]
    fn no_threshold_eliminates_nothing() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let host = rect(0.0, 0.0, 1.0, 1.0);
        let speck = rect(0.0, 0.4, 0.1, 0.5);
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, speck)], &classes, 0.0, None);
        assert_eq!(stats.eliminated, 0, "{stats:?}");
        assert!(out.iter().any(|(sid, _, _)| *sid == 2), "the speck keeps its own class");
        assert!((area(&out) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn absorption_follows_the_longest_shared_boundary() {
        let classes = [fill_style(1, 0, 0x0001), fill_style(2, 1, 0x0002), fill_style(3, 5, 0x0003)].map(|s| s);
        let classes = merge_classes(&classes);
        let west = poly(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        let east = poly(&[(1.1, 0.0), (2.0, 0.0), (2.0, 1.0), (1.1, 1.0)]);
        let sliver = poly(&[(1.0, 0.0), (1.1, 0.0), (1.1, 0.02), (1.1, 1.0), (1.0, 1.0)]);
        let mpp = 100.0;
        let e = Some(threshold_for(0.5, mpp));
        let (out, stats) = coverage_simplify_fills(vec![(1, west), (2, east), (3, sliver)], &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.eliminated, 1, "{stats:?}");
        let by = |sid: u8| area(&out.iter().filter(|(s, _, _)| *s == sid).cloned().collect::<Vec<_>>());
        assert!((by(1) - 1.1).abs() < 1e-9, "west swallowed the sliver: {out:?}");
        assert!((by(2) - 0.9).abs() < 1e-9, "east is untouched: {out:?}");
        assert!(by(3) < 1e-12, "and the sliver's class is gone from the tier: {out:?}");
    }

    #[test]
    fn elimination_never_fills_a_large_uncovered_face() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let mpp = 100.0;
        let (out, stats) = coverage_simplify_fills(ring_around_a_hole(), &classes, 0.0, Some(threshold_for(0.5, mpp)));
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "a 1 deg² gap under a 0.5 deg² threshold is not a micro-gap: {stats:?}");
        assert!(stats.dropped_faces >= 1, "the centre still belongs to nobody: {stats:?}");
        assert!((area(&out) - 9.0).abs() < 1e-9, "nine squares in, nine squares of ground out");
        assert_eq!(hole_count(&out), 1, "the unmapped centre survives as a hole rather than being invented into fill");
    }

    /// A 10x10 fill with an unmapped `w` x `h` hole at its centre, plus a neighbour of a second
    /// class so the component is a real arrangement. Polygonize turns the hole into its own face,
    /// which nothing covers — the shape of every micro-gap decimation can open.
    fn host_with_hole(w: f64, h: f64) -> Vec<(u8, Geom)> {
        let (x0, x1) = (5.0 - w * 0.5, 5.0 + w * 0.5);
        let (y0, y1) = (5.0 - h * 0.5, 5.0 + h * 0.5);
        let host = Geom::Polygon {
            exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]],
        };
        vec![(1u8, host), (2u8, rect(10.0, 0.0, 11.0, 1.0))]
    }

    /// The tier the healing tests run at: `tol` 0.08 deg decimates at 0.01 deg, so the sliver bound
    /// is `HEAL_WIDTH_TOLERANCES` x 0.01 deg of mean half-width.
    const HEAL_TOL: f64 = 0.08;

    #[test]
    fn a_decimation_scale_sliver_is_healed() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        // 4° x 0.03°: mean half-width ~0.0149°, under the 0.02° bound. Area 0.12°², under the 0.5°²
        // threshold and far under the 1°² neighbour.
        let e = Some(threshold_for(0.5, 100.0));
        let (out, stats) = coverage_simplify_fills(host_with_hole(4.0, 0.03), &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 1, "the crack was healed: {stats:?}");
        assert_eq!(stats.dropped_faces, 0, "and nothing is left uncovered: {stats:?}");
        assert!(area(&out) > 100.0, "the ground is whole: {}", area(&out));
    }

    #[test]
    fn a_compact_gap_is_not_healed_even_far_below_the_threshold() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        // 0.35° x 0.35°: area 0.122°² — within a percent of the sliver's — but mean half-width
        // 0.087°, more than four times the bound.
        let e = Some(threshold_for(0.5, 100.0));
        let (_out, stats) = coverage_simplify_fills(host_with_hole(0.35, 0.35), &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "a compact gap is geography, not a crack: {stats:?}");
        assert_eq!(stats.dropped_faces, 1, "and it stays a gap: {stats:?}");
    }

    #[test]
    fn a_sliver_over_the_area_threshold_is_not_healed() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let e = Some(threshold_for(0.05, 100.0));
        let (_out, stats) = coverage_simplify_fills(host_with_hole(4.0, 0.03), &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "over the threshold, however thin: {stats:?}");
        assert_eq!(stats.dropped_faces, 1, "{stats:?}");
    }

    #[test]
    fn mean_half_width_separates_thin_from_small() {
        let ribbon = rect(0.0, 0.0, 4.0, 0.02); // width 0.02° ⇒ 0.01° ⇒ ~1113 m
        let square = rect(0.0, 0.0, 0.3, 0.3); // side 0.3° ⇒ 0.075° ⇒ ~8349 m
        let w = sliver_half_width_m(&ribbon);
        let q = sliver_half_width_m(&square);
        assert!((w - 0.01 * M_PER_DEG).abs() < 0.02 * M_PER_DEG * 0.05, "a ribbon reports half its width: {w}");
        assert!((q - 0.075 * M_PER_DEG).abs() < 0.075 * M_PER_DEG * 0.05, "a square a quarter of its side: {q}");
        assert!(w < q, "and the ribbon is the thin one even though it has 8x the area");
        assert_eq!(
            sliver_half_width_m(&Geom::Line(vec![(0.0, 0.0), (1.0, 1.0)])),
            f64::INFINITY,
            "a line is never a sliver"
        );
    }

    #[test]
    fn a_gap_never_absorbs_a_covered_face() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let host = Geom::Polygon {
            exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![vec![(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0), (2.0, 2.0)]],
        };
        let island = rect(4.0, 4.0, 6.0, 6.0); // 4 deg², wholly inside the hole
                                               // 2 deg²: under the island, the 32 deg² hole and the 64 deg² ring around it.
        let e = Some(threshold_for(2.0, 100.0));
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, island)], &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "the hole is nowhere near thin: {stats:?}");
        assert_eq!(stats.uneliminable_culled, 0, "the island is over the threshold: {stats:?}");
        let by = |sid: u8| area(&out.iter().filter(|(s, _, _)| *s == sid).cloned().collect::<Vec<_>>());
        assert!(by(2) > 3.5, "the island kept its fill instead of being eaten by the gap: {out:?}");
        assert!((by(1) - 64.0).abs() < 1e-9, "and the host did not grow into the hole either: {out:?}");
    }

    #[test]
    fn a_covered_speck_with_no_covered_neighbour_is_culled() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let host = Geom::Polygon {
            exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![vec![(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0), (2.0, 2.0)]],
        };
        let speck = rect(4.0, 4.0, 5.0, 5.0); // 1 deg², wholly inside the hole
        let e = Some(threshold_for(2.0, 100.0));
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, speck)], &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.eliminated, 0, "there was nothing to absorb it into: {stats:?}");
        assert_eq!(stats.uneliminable_culled, 1, "so the cull took it: {stats:?}");
        let by = |sid: u8| area(&out.iter().filter(|(s, _, _)| *s == sid).cloned().collect::<Vec<_>>());
        assert_eq!(by(2), 0.0, "the speck is gone: {out:?}");
        assert!((by(1) - 64.0).abs() < 1e-9, "and its ground went to the backdrop, not to the host: {out:?}");
    }

    #[test]
    fn same_class_parcels_are_dissolved_before_the_arrangement() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let mut feats: Vec<(u8, Geom)> = (0..10).map(|i| (1u8, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        feats.push((2, rect(0.0, 1.0, 10.0, 2.0))); // a second class above, so an arrangement is built
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, None);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.inputs, 11, "eleven fills arrived: {stats:?}");
        assert_eq!(stats.dissolved, 2, "and two polygons reached the arrangement: {stats:?}");
        assert_eq!(stats.faces, 2, "one face per class, not one per parcel: {stats:?}");
        assert!((area(&out) - 20.0).abs() < 1e-9, "the ground is untouched: {}", area(&out));
    }

    #[test]
    fn the_pre_dissolve_leaves_single_member_classes_alone() {
        let styles: Vec<Style> = (1..=3).map(|i| fill_style(i, i as i8, 0x0001 + i as u16)).collect();
        let classes = merge_classes(&styles);
        let feats: Vec<(u8, Geom)> = (0..3).map(|i| (i as u8 + 1, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, None);
        assert_eq!((stats.inputs, stats.dissolved), (3, 3), "nothing to dissolve: {stats:?}");
        assert!((area(&out) - 3.0).abs() < 1e-9);
    }

    fn a_fill(seq: usize, style_id: u8, g: Geom) -> Fill {
        let bounds = g.bounds();
        Fill { seq, style_id, canonical: style_id, key: (0, 0, 0), geom: g, bounds }
    }

    #[test]
    fn a_fill_set_id_sees_composition_and_geometry() {
        let base = vec![a_fill(0, 1, rect(0.0, 0.0, 1.0, 1.0)), a_fill(3, 2, rect(2.0, 0.0, 3.0, 1.0))];
        let id = FillSetId::of(&base);
        assert!(id == FillSetId::of(&base), "the same set is the same id");

        let moved = vec![a_fill(0, 1, rect(0.0, 0.0, 1.0, 1.0)), a_fill(3, 2, rect(2.0, 0.0, 3.0, 1.001))];
        assert!(id != FillSetId::of(&moved), "a moved vertex is a different set");

        let restyled = vec![a_fill(0, 1, rect(0.0, 0.0, 1.0, 1.0)), a_fill(3, 9, rect(2.0, 0.0, 3.0, 1.0))];
        assert!(id != FillSetId::of(&restyled), "a different style id is a different set");

        let reseq = vec![a_fill(0, 1, rect(0.0, 0.0, 1.0, 1.0)), a_fill(4, 2, rect(2.0, 0.0, 3.0, 1.0))];
        assert!(id != FillSetId::of(&reseq), "a different position in the tier is a different set");

        assert!(id != FillSetId::of(&base[..1]), "a shorter set is a different set");
    }

    #[test]
    fn the_predissolve_cache_misses_on_a_different_fill_set() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let coarse = || vec![(1u8, rect(0.0, 0.0, 1.0, 1.0)), (1u8, rect(1.0, 0.0, 2.0, 1.0))];
        let fine = || {
            let mut v = coarse();
            v.push((2u8, rect(2.0, 0.0, 3.0, 1.0)));
            v
        };
        let key = |v: &[(u8, Geom, bool)]| v.iter().map(|(s, g, d)| (*s, verts(g), *d)).collect::<Vec<_>>();

        let shared = PredissolveCache::new();
        let (_, c0) = coverage_simplify_fills_with(coarse(), &classes, 0.0, None, &shared, &Progress::silent());
        let (warm, c1) = coverage_simplify_fills_with(fine(), &classes, 0.0, None, &shared, &Progress::silent());
        let (cold, c2) =
            coverage_simplify_fills_with(fine(), &classes, 0.0, None, &PredissolveCache::new(), &Progress::silent());

        assert_eq!(c0.dissolved, 1, "the coarse tier dissolved to one polygon: {c0:?}");
        assert_eq!(c1, c2, "the fine tier's counters do not depend on what the cache held");
        assert_eq!(key(&warm), key(&cold), "nor its geometry");
    }

    #[test]
    fn the_predissolve_cache_changes_nothing_it_serves() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let build = || {
            let mut v: Vec<(u8, Geom)> = (0..6).map(|i| (1u8, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
            v.push((2, rect(0.0, 1.0, 6.0, 2.0)));
            v
        };
        let key = |v: &[(u8, Geom, bool)]| v.iter().map(|(s, g, d)| (*s, verts(g), *d)).collect::<Vec<_>>();
        let shared = PredissolveCache::new();
        let (a, sa) = coverage_simplify_fills_with(build(), &classes, 0.05, None, &shared, &Progress::silent());
        let (b, sb) =
            coverage_simplify_fills_with(build(), &classes, 0.05, None, &PredissolveCache::new(), &Progress::silent());
        assert_eq!(sa, sb, "same counters warm or cold");
        assert_eq!(key(&a), key(&b), "same geometry warm or cold");

        shared.clear();
        let (c, sc) = coverage_simplify_fills_with(build(), &classes, 0.05, None, &shared, &Progress::silent());
        assert_eq!(sa, sc, "and clearing it is not observable either");
        assert_eq!(key(&a), key(&c));
    }

    #[test]
    fn the_decimation_tolerance_is_a_small_fraction_of_the_tier() {
        assert_eq!(decimation_tol(0.0), 0.0, "no simplify ⇒ no decimation");
        let coarse = 2200.0 / M_PER_DEG; // the shipping ladder's LOD 0
        assert!((decimation_tol(coarse) - coarse / 8.0).abs() < 1e-15, "an eighth of the tier");
        let floor = DECIMATE_FLOOR_M / M_PER_DEG;
        let just_over = floor * 4.0; // /8 would be under the floor
        assert!((decimation_tol(just_over) - floor).abs() < 1e-15, "the floor binds");
        let fine = floor / 2.0;
        assert!((decimation_tol(fine) - fine).abs() < 1e-15, "and never coarser than the tier's own tolerance");
    }

    #[test]
    fn decimation_thins_the_arrangement_input() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 3, 0x0002)]);
        // Two abutting slabs whose shared edge is a dense sawtooth: 400 teeth of 0.0005 deg, far
        // under the 0.01 deg tier tolerance below.
        let teeth = 400;
        let seam: Vec<(f64, f64)> = (0..=teeth)
            .map(|i| {
                let t = i as f64 / teeth as f64;
                (1.0 + if i % 2 == 0 { 0.0 } else { 0.0005 }, t)
            })
            .collect();
        let mut west = vec![(0.0, 0.0), (1.0, 0.0)];
        west.extend(seam.iter().copied());
        west.extend([(0.0, 1.0)]);
        let mut east = vec![(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
        east.extend(seam.iter().rev().skip(1).copied());
        // A threshold well under either slab, so nothing real is eliminated; it is here because it
        // is what unlocks the pre-pass.
        let e = Some(threshold_for(0.1, 100.0));
        let (out, stats) = coverage_simplify_fills(vec![(1, poly(&west)), (2, poly(&east))], &classes, 0.01, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!(stats.vertices_in > 800, "the fixture really is detailed: {stats:?}");
        assert!(
            stats.vertices_arranged * 8 < stats.vertices_in,
            "the arrangement saw an order of magnitude fewer vertices: {stats:?}"
        );
        // The sawtooth is 0.0005 deg deep on a 2 deg² pair: the ground it moves is noise.
        assert!((area(&out) - 2.0).abs() < 0.01, "and the ground is the same to within the tolerance: {}", area(&out));
        assert_eq!(stats.dropped_faces, 0, "and the two slabs are still glued, not torn: {stats:?}");
        let seam_of = |sid: u8| {
            let (_, g, _) = out.iter().find(|(s, _, _)| *s == sid).expect("both classes survive");
            seam_verts(g)
        };
        assert_eq!(seam_of(1), seam_of(2), "the shared boundary is the SAME vertices on both sides");
    }

    #[test]
    fn decimation_keeps_the_shared_boundary_shared() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 0, 0x0002)]);
        let e = Some(threshold_for(0.5, 100.0)); // under both fills; only there to unlock the pre-pass
        let (out, stats) = coverage_simplify_fills(vec![(1, seam_west()), (2, seam_east())], &classes, 0.02, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        let seam_of = |sid: u8| {
            let (_, g, _) = out.iter().find(|(s, _, _)| *s == sid).expect("both classes survive");
            seam_verts(g)
        };
        let (a, b) = (seam_of(1), seam_of(2));
        assert!(!a.is_empty(), "the seam did not vanish");
        assert_eq!(a, b, "the shared boundary must still be the SAME vertices on both sides");
    }

    #[test]
    fn a_chain_of_equal_specks_coalesces_completely() {
        let styles: Vec<Style> = (1..=8).map(|i| fill_style(i, i as i8, 0x0001 + i as u16)).collect();
        let classes = merge_classes(&styles);
        let feats: Vec<(u8, Geom)> = (0..8).map(|i| (i as u8 + 1, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        let mpp = 100.0;
        let e = Some(threshold_for(7.5, mpp));
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.uneliminable_culled, 0, "the coalesced row cleared the threshold: {stats:?}");
        assert!((area(&out) - 8.0).abs() < 1e-9, "all eight tiles of ground survive: {out:?}");
        let live: std::collections::BTreeSet<u8> = out.iter().map(|(sid, _, _)| *sid).collect();
        assert_eq!(live.len(), 1, "the row coalesced into a single class, not four pairs: {live:?}");
        assert_eq!(out.len(), 1, "and into a single polygon: {out:?}");
    }

    #[test]
    fn a_chain_that_cannot_reach_the_threshold_is_culled_whole() {
        let styles: Vec<Style> = (1..=8).map(|i| fill_style(i, i as i8, 0x0001 + i as u16)).collect();
        let classes = merge_classes(&styles);
        let feats: Vec<(u8, Geom)> = (0..8).map(|i| (i as u8 + 1, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        let e = Some(threshold_for(100.0, 100.0));
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.uneliminable_culled, 8, "every face of the one dead-end cluster: {stats:?}");
        assert_eq!(area(&out), 0.0, "nothing is emitted: {out:?}");
    }

    #[test]
    fn elimination_is_deterministic() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 3, 0x0002), fill_style(3, 5, 0x0003)]);
        let build = || {
            let mut v: Vec<(u8, Geom)> = vec![(1, rect(0.0, 0.0, 12.0, 2.0))];
            for i in 0..12 {
                let x = i as f64;
                v.push((2, rect(x + 0.1, 0.1, x + 0.2, 0.2)));
                v.push((3, rect(x + 0.4, 0.4, x + 0.45, 0.9)));
            }
            v
        };
        let mpp = 100.0;
        let e = Some(threshold_for(1.0, mpp));
        let (a, sa) = coverage_simplify_fills(build(), &classes, 0.05, e);
        let (b, sb) = coverage_simplify_fills(build(), &classes, 0.05, e);
        assert_eq!(sa, sb, "same counters");
        assert!(sa.eliminated >= 24, "every speck was absorbed: {sa:?}");
        let key = |v: &[(u8, Geom, bool)]| v.iter().map(|(s, g, done)| (*s, verts(g), *done)).collect::<Vec<_>>();
        assert_eq!(key(&a), key(&b), "same style ids, same vertices, same order");
    }

    #[test]
    fn broken_geometry_sits_out_without_losing_features() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        let stub = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], interiors: vec![] };
        let bowtie = poly(&[(0.1, 0.1), (0.9, 0.9), (0.9, 0.1), (0.1, 0.9)]);
        let neighbour = rect(0.0, 0.0, 1.0, 1.0); // same bbox ⇒ same component as both
        let (out, stats) =
            coverage_simplify_fills(vec![(1, stub.clone()), (1, bowtie.clone()), (1, neighbour)], &classes, 0.2, None);
        assert_eq!(stats.fallbacks, 0, "the component itself was fine: {stats:?}");
        assert_eq!(out.len(), 3, "every feature is still there");
        for broken in [&stub, &bowtie] {
            let kept = out
                .iter()
                .find(|(_, g, _)| verts(g) == verts(broken))
                .unwrap_or_else(|| panic!("the broken shape must come back untouched: {out:?}"));
            assert!(!kept.2, "and unsimplified, for the per-feature path to handle");
        }
        assert_eq!(
            out.iter().filter(|(_, _, simplified)| *simplified).count(),
            1,
            "the valid neighbour still went through the coverage path"
        );
    }

    #[test]
    fn a_component_of_only_broken_geometry_falls_back() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        let stub = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], interiors: vec![] };
        let far = rect(10.0, 10.0, 11.0, 11.0); // its own component, unaffected
        let (out, stats) = coverage_simplify_fills(vec![(1, stub.clone()), (1, far)], &classes, 0.2, None);
        assert_eq!(stats.fallbacks, 1, "exactly the broken component fell back: {stats:?}");
        assert_eq!(out.len(), 2, "nothing was dropped");
        assert!(out.iter().any(|(s, g, done)| (*s, verts(g), *done) == (1, verts(&stub), false)));
        assert!(out.iter().any(|(_, _, simplified)| *simplified), "the far component still got the coverage path");
    }

    #[test]
    fn outlined_polygons_and_lines_pass_through() {
        let styles = [fill_style(1, 0, 0x0001), Style { color2: Some(0x1234), ..fill_style(2, 0, 0x0002) }];
        let classes = merge_classes(&styles);
        let line = Geom::Line(vec![(0.0, 0.0), (1.0, 1.0)]);
        let cased = rect(0.0, 0.0, 1.0, 1.0);
        let (out, stats) = coverage_simplify_fills(
            vec![(2, cased.clone()), (1, line.clone()), (1, rect(0.0, 0.0, 1.0, 1.0))],
            &classes,
            0.2,
            None,
        );
        assert_eq!(stats.inputs, 1, "only the plain fill participates: {stats:?}");
        let cased_out = out.iter().find(|(s, _, _)| *s == 2).expect("the cased polygon survives");
        assert_eq!(verts(&cased_out.1), verts(&cased), "untouched");
        assert!(!cased_out.2, "and still needs the per-feature simplify");
        assert!(out.iter().any(|(_, g, _)| matches!(g, Geom::Line(_))), "the line survives as a line");
    }

    #[test]
    fn the_same_input_twice_is_the_same_output() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 3, 0x0002), fill_style(3, 1, 0x0003)]);
        let build = || {
            let mut v: Vec<(u8, Geom)> = Vec::new();
            for i in 0..12 {
                let x = i as f64;
                v.push((1, rect(x, 0.0, x + 1.0, 1.0)));
                v.push((2, rect(x + 0.5, 0.5, x + 1.5, 1.5)));
                v.push((3, rect(x, 3.0, x + 1.0, 4.0)));
            }
            v
        };
        let (a, sa) = coverage_simplify_fills(build(), &classes, 0.05, None);
        let (b, sb) = coverage_simplify_fills(build(), &classes, 0.05, None);
        assert_eq!(sa, sb, "same counters");
        let key = |v: &[(u8, Geom, bool)]| v.iter().map(|(s, g, done)| (*s, verts(g), *done)).collect::<Vec<_>>();
        assert_eq!(key(&a), key(&b), "same style ids, same vertices, same order");
    }

    #[test]
    fn a_class_is_emitted_at_its_first_members_position() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        let feats = vec![
            (1u8, rect(0.0, 0.0, 1.0, 1.0)), // A, class 1
            (9u8, rect(0.0, 5.0, 1.0, 6.0)), // B, no class
            (1u8, rect(1.0, 0.0, 2.0, 1.0)), // C, class 1, abuts A
        ];
        let (out, _) = coverage_simplify_fills(feats, &classes, 0.0, None);
        assert_eq!(out.len(), 2, "A+C dissolved into one, B passthrough: {out:?}");
        assert_eq!(out[0].0, 1, "the class block sits at A's position");
        assert_eq!(out[1].0, 9, "B keeps its place after it");
    }
}
