//! `coverage.rs` — the shared-boundary simplify pass: treat a tier's plain fills as one
//! **polygonal coverage** and simplify every shared edge exactly **once**, so neighbours stay
//! glued at any tolerance.
//!
//! # The tearing this fixes
//!
//! Today each feature is simplified on its own ([`crate::geom::topology_preserve_simplify`]).
//! Two abutting fills of *different* render classes — a farmland against a wood, a lake against
//! its shoreline landuse — carry two copies of the same OSM boundary, and an independent
//! simplify moves each copy its own way. At a coarse tier's tolerance the two walk apart by up
//! to `simplify_m` metres and the backdrop shows through the gap: a sliver of nothing where the
//! map was continuous. [`merge_fills`](crate::merge::merge_fills) only dissolves boundaries
//! *inside* one class, so it cannot see this seam at all.
//!
//! The fix is to stop simplifying features and start simplifying the **arrangement**:
//!
//! 1. Collect the tier's participating fills — exactly `merge_fills`' notion of a plain fill,
//!    a polygon whose style carries no `color2` (a `color2` strokes the rings, and a stroked
//!    wall is a thing you can see, so it is never dissolved or re-cut).
//! 2. Node every boundary into one planar arrangement and polygonize it into **faces**.
//! 3. Give each face to the fill that would be *visible* there under the device paint order —
//!    spans paint by `(z_index, seq)` and later paints over — so an overlap resolves to the
//!    class on top and the hidden part is simply gone. A face no fill covers is dropped: the
//!    backdrop showing through a genuine gap is correct, and inventing a fill for it would be
//!    the one failure worse than tearing.
//! 4. Dissolve the faces per class (`GEOSCoverageUnion`, cheap — the edges already match).
//! 5. Hand **every class's polygons together** to `GEOSCoverageSimplifyVW` in one call, so an
//!    edge shared by two classes is simplified once and both sides keep the identical vertex
//!    sequence. That is the whole point; splitting the call per class would reintroduce the
//!    tear it exists to remove.
//!
//! Everything downstream — the footprint cull, the sub-pixel hole trim, the quadtree — runs
//! unchanged on the result, and the OBCM bytes are the same shape as ever: this is a bake-time
//! geometry transform, not a format change.
//!
//! # Components
//!
//! The arrangement is not built over a whole country at once. Fills are first split into
//! **bbox-connected components**: if two polygons' bounding boxes do not intersect they cannot
//! share an edge or overlap, so their arrangements are independent and a per-component pass is
//! *identical* to a global one — while costing a fraction of it, and running in parallel
//! (nothing GEOS-owned crosses a thread, exactly as in [`crate::geom::union_polygons`]).
//!
//! What it costs is a planar overlay, and wall-to-wall landuse really is one component: a
//! synthetic 90 000-parcel cluster (24 vertices each, all edges shared) takes ~70 s and ~3.1 GB
//! on an M-series laptop, against ~3 s and a few hundred MB for 10 000. That is why this is a
//! per-tier knob rather than a global one — the coarse tiers, where the tolerance is metres wide
//! and the tearing is what you actually see, hold a fraction of the fills the fine ones do.
//!
//! # Eliminating small faces instead of dropping them
//!
//! A coarse tier has to shed detail, and the ordinary way to do that is
//! [`crate::geom::footprint_below`]: a polygon under the tier's `min_area_px` is dropped. On a
//! *coverage* tier that is the wrong operator. The fills there tile the ground, so dropping one
//! punches a hole in the tiling and the backdrop shows through — the same failure as tearing,
//! arrived at deliberately. Worse, the low-`z` base fill (`natural.land`) ends up owning every
//! scrap of ground no landuse claims, so a coarse tier renders as lace.
//!
//! So on a coverage tier `min_area_px` is an **elimination** threshold rather than a drop
//! threshold — the cartographic operator of the same name. A face below it is not deleted; it is
//! given to the neighbouring face it shares the **longest boundary** with, and the per-class
//! dissolve below then swallows it whole. Coverage stays complete, the class that was too small
//! to see disappears into the one around it, and both the face count and the vertex count fall,
//! because an absorbed face's boundary stops existing instead of being simplified. It runs to a
//! **fixed point**: absorbing grows the survivor, so a cluster of specks coalesces outward step by
//! step and the pass ends with nothing under the threshold left — where one sweep would leave every
//! speck settled on the speck next door and the threshold binding nothing at all.
//!
//! The caller's cull is then skipped for everything this pass produced (see
//! [`coverage_simplify_fills_with`]'s return contract) — it has already been applied, in the one
//! form a coverage can survive.
//!
//! # Never drop map content
//!
//! Any GEOS failure — a boundary that will not node, a polygonize that returns nothing, a
//! coverage the validity check refuses — falls that **component** back to the ordinary
//! per-feature path: its fills come out unchanged, tagged "not simplified", and the caller's
//! usual simplify handles them. Other components keep the coverage treatment; a component's
//! neighbours are, by construction, nobody. Individual **invalid** input polygons do not even
//! cost that: they sit the arrangement out and pass through, so one self-intersecting parcel
//! cannot un-glue a whole cluster (see [`coverage_component`]).

use std::collections::{BTreeMap, HashMap};

use geos::{Geom as _, Geometry, PreparedGeometry, STRtree, SpatialIndex};
use rayon::prelude::*;

use obc_map_scene::M_PER_DEG;

use crate::geom::{
    box_polygon, collect_polygons, coverage_is_valid, coverage_simplify_vw, footprint_area_px, from_geos,
    try_polygon_to_geos, Bounds, Geom,
};
use crate::merge::ClassKey;
use crate::progress::Progress;

/// What the pass did to one LOD, for the per-tier log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CoverageStats {
    /// Participating fill polygons consumed.
    pub inputs: usize,
    /// Polygons emitted in their place.
    pub outputs: usize,
    /// Bbox-connected components the arrangement was built over.
    pub components: usize,
    /// Faces the arrangements produced.
    pub faces: usize,
    /// Faces no fill covered — genuine gaps, dropped rather than invented.
    pub dropped_faces: usize,
    /// Faces below the tier's threshold that were absorbed into a neighbour (see the module docs).
    pub eliminated: usize,
    /// Class groups whose `GEOSCoverageUnion` refused, leaving that class's faces undissolved.
    pub dissolve_failures: usize,
    /// Components that hit a GEOS failure and fell back to the per-feature path.
    pub fallbacks: usize,
}

/// The tier's small-face elimination threshold: `min_area_px` square pixels at `mpp`
/// meters-per-pixel — the same pair [`crate::geom::footprint_below`] culls with, applied as the
/// absorb-into-a-neighbour operator the module docs describe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eliminate {
    /// The scale the threshold is measured at (the next-finer tier's `max_mpp`).
    pub mpp: f64,
    /// Faces below this many square pixels are absorbed.
    pub min_area_px: f64,
}

impl Eliminate {
    /// The pair, if both halves are usable — `None` disables elimination entirely.
    pub fn new(mpp: Option<f64>, min_area_px: f64) -> Option<Self> {
        let mpp = mpp?;
        (mpp > 0.0 && min_area_px > 0.0).then_some(Eliminate { mpp, min_area_px })
    }
}

/// A participating fill: one polygon of one plain-fill feature.
struct Fill {
    /// The feature's position in the tier's input order — the paint-order tiebreak.
    seq: usize,
    /// The style the polygon arrived with (what a fallback re-emits).
    style_id: u8,
    /// The class's canonical (smallest) style id — what a coverage result is tagged with.
    canonical: u8,
    /// `(z_index, color, priority)`; `z_index` is the paint-order key.
    key: ClassKey,
    geom: Geom,
    bounds: Bounds,
}

/// One emission slot in input order (the same device [`crate::merge`] uses): a passthrough
/// feature, or a class's coverage output emitted at its **first member's** position.
enum Slot {
    Pass(u8, Geom),
    Group(u8),
}

impl Slot {
    /// The emitted feature of a passthrough slot. A `Group` slot only exists where a fill joined
    /// it, so it never reaches here; an empty geometry is the harmless answer if it ever did (the
    /// quadtree drops empties).
    fn into_pass(self) -> (u8, Geom, bool) {
        match self {
            Slot::Pass(sid, g) => (sid, g, false),
            Slot::Group(sid) => (sid, Geom::Empty, false),
        }
    }
}

/// Coverage-simplify a tier's plain fills.
///
/// `features` is the tier's `(style_id, geom)` list after the `min_lod` filter, `classes` the
/// [`crate::merge::merge_classes`] table (a style is a plain fill iff it is in there), and `tol`
/// the tier's simplify tolerance **in degrees** (`simplify_m / M_PER_DEG`; `0.0` ⇒ dissolve and
/// re-cut, but do not simplify).
///
/// Returns `(style_id, geom, simplified)` in slot order. `simplified == true` means the pass
/// already applied the tier's tolerance and the caller must **not** simplify it again;
/// `false` marks everything that took the ordinary path — lines, outlined polygons, styles in
/// no class, and every fill of a component that fell back.
pub fn coverage_simplify_fills(
    features: Vec<(u8, Geom)>,
    classes: &HashMap<u8, (ClassKey, u8)>,
    tol: f64,
    eliminate: Option<Eliminate>,
) -> (Vec<(u8, Geom, bool)>, CoverageStats) {
    coverage_simplify_fills_with(features, classes, tol, eliminate, &Progress::silent())
}

/// [`coverage_simplify_fills`], abandonable.
///
/// The checkpoint is per component, for the same reason [`crate::merge::merge_fills_with`] puts
/// one per group: an arrangement over a big cluster runs for seconds inside GEOS and cannot be
/// interrupted from outside. A cancelled component takes the fallback path — the same one a GEOS
/// failure takes, so a cancelled run's output stays well-formed instead of becoming a case
/// nobody tests. The work is discarded anyway; the point is only to stop starting more of it.
pub fn coverage_simplify_fills_with(
    features: Vec<(u8, Geom)>,
    classes: &HashMap<u8, (ClassKey, u8)>,
    tol: f64,
    eliminate: Option<Eliminate>,
    progress: &Progress,
) -> (Vec<(u8, Geom, bool)>, CoverageStats) {
    // --- Phase 1: lay out slots and collect the participants, both in input order. ---
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
    if fills.is_empty() {
        // Nothing participated (a lines-only tier, or one whose polygons are all outlined): the
        // input echoes back untouched, and there are no `Group` slots to fill.
        return (slots.into_iter().map(|s| s.into_pass()).collect(), stats);
    }

    // --- Phase 2: bbox-connected components (see the module docs). ---
    let components = bbox_components(&fills);
    stats.components = components.len();

    // --- Phase 3: one arrangement per component, in parallel. Every GEOS object a task
    // touches is built, used and dropped on that task's own thread (`geos::Geometry` is
    // `!Send`); only plain `Geom` crosses a thread boundary. ---
    let results: Vec<Option<ComponentOut>> = components
        .par_iter()
        .map(|comp| if progress.is_cancelled() { None } else { coverage_component(&fills, comp, tol, eliminate) })
        .collect();

    // --- Phase 4: emit in slot order, each class's polygons at its first member's position. ---
    let mut by_class: HashMap<u8, Vec<(u8, Geom, bool)>> = HashMap::new();
    for (comp, result) in components.iter().zip(results) {
        match result {
            Some(out) => {
                stats.faces += out.faces;
                stats.dropped_faces += out.dropped_faces;
                stats.eliminated += out.eliminated;
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

/// Split a geometry into its polygon parts (coverage candidates) and everything else (lines
/// pass through). The [`crate::merge`] splitter, verbatim in behaviour: flatten `Multi`, drop
/// `Empty`.
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
    /// `(style id, polygon, simplified)`: the coverage output in class order then GEOS order,
    /// followed by the members that sat the arrangement out (see [`coverage_component`]).
    /// Deterministic.
    polys: Vec<(u8, Geom, bool)>,
    faces: usize,
    dropped_faces: usize,
    eliminated: usize,
    dissolve_failures: usize,
}

/// Partition fills into connected components under **bounding-box intersection**.
///
/// Two polygons that share an edge or overlap necessarily have intersecting boxes, so this is a
/// conservative superset of "geometrically interacting" — every interaction stays inside one
/// component, and two different components are provably disjoint (if their boxes met they would
/// be one component). The linking pairs come from a GEOS `STRtree` over the boxes, so this is
/// `O(n log n)` rather than the `O(n²)` of comparing every pair.
///
/// Deterministic regardless of query order: a union-find decides membership, and the groups are
/// then read off by walking `0..n`, so each component's members ascend and the components
/// themselves are ordered by their smallest member. A tree that will not build degenerates to
/// one component containing everything — correct, merely slower.
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
    // Envelopes as GEOS boxes. A box that will not build (a degenerate bound) links to nothing
    // by itself, so the polygon simply forms its own component and is coverage-simplified alone.
    let boxes: Vec<Option<Geometry>> = fills.iter().map(|f| box_polygon(f.bounds).ok()).collect();
    if let Ok(mut tree) = STRtree::<usize>::with_capacity(n.max(1)) {
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
    // Group by root, ascending within and across groups.
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

/// The whole pass for one component: arrangement → face assignment → per-class dissolve →
/// one coverage simplify. `None` on any GEOS failure, which the caller turns into a
/// pass-through of this component's fills.
///
/// **Invalid members sit it out.** Real OSM occasionally arrives with a self-intersecting ring or
/// a hole no shell contains (the DACH bake's first casualty — see [`crate::geom::clip_to_box`]),
/// and GEOS answers point-in-polygon questions about such a shape however it likes. Rather than
/// let one broken parcel silently delete itself (a face nobody is found to cover is *dropped*,
/// which is right for a gap and catastrophic for a mis-answered test), an invalid member is kept
/// out of the arrangement and passed through to the ordinary per-feature path. Its valid
/// neighbours still get glued to each other; the worst case is that the broken shape overlaps a
/// face it used to own, which is exactly what the packer stores today.
fn coverage_component(fills: &[Fill], comp: &[usize], tol: f64, eliminate: Option<Eliminate>) -> Option<ComponentOut> {
    // The members as GEOS polygons — also the inputs of the point-in-polygon assignment.
    let mut members: Vec<Geometry> = Vec::with_capacity(comp.len());
    let mut member_of: Vec<usize> = Vec::with_capacity(comp.len());
    let mut sat_out: Vec<(u8, Geom, bool)> = Vec::new();
    for (k, &i) in comp.iter().enumerate() {
        match try_polygon_to_geos(&fills[i].geom) {
            Some(g) if g.is_valid().unwrap_or(false) => {
                members.push(g);
                member_of.push(k);
            }
            _ => sat_out.push((fills[i].style_id, fills[i].geom.clone(), false)),
        }
    }
    if members.is_empty() {
        return None;
    }

    // --- Faces. A lone fill is its own arrangement (nothing to node against), which skips the
    // overlay machinery for the common isolated polygon exactly as `union_polygons` does.
    //
    // `members` is scoped to this block on purpose: on a wall-to-wall landuse cluster it is tens
    // of thousands of live GEOS polygons, and the dissolve and simplify below have no use for
    // them. Freeing them here keeps the component's two heavy phases from overlapping in peak
    // memory. ---
    let (faces, mut winners) = {
        let members = members; // moved in, so the block's end frees them
        if members.len() == 1 {
            (vec![fills[comp[member_of[0]]].geom.clone()], vec![Some(0usize)])
        } else {
            let faces = arrangement_faces(&members)?;
            let winners = assign_faces(&faces, &members, fills, comp, &member_of)?;
            (faces, winners)
        }
    };
    let n_faces = faces.len();
    let dropped = winners.iter().filter(|w| w.is_none()).count();

    // --- Elimination: a face under the tier's threshold joins the neighbour it shares the most
    // boundary with, so the dissolve below absorbs it instead of the cull deleting it. ---
    let eliminated = match eliminate {
        Some(e) => eliminate_small_faces(&faces, &mut winners, e),
        None => 0,
    };

    // --- Per-class dissolve, classes in key order so emission is deterministic. ---
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
            dissolve_failures,
        });
    }

    // --- One coverage simplify over every class at once: the shared edges are simplified once
    // and both sides come back with the identical vertex sequence. ---
    let mut polys: Vec<(u8, Geom, bool)> = Vec::new();
    if tol > 0.0 {
        let refs: Vec<&Geom> = elements.iter().collect();
        // The simplifier assumes a valid coverage; on anything else its output is not glued and
        // not trustworthy, so refuse rather than ship a subtly torn tier.
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
    Some(ComponentOut { polys, faces: n_faces, dropped_faces: dropped, eliminated, dissolve_failures })
}

/// Node every member's boundary into one planar arrangement and polygonize it into faces.
///
/// Noding is what makes the edges *shared*: after it, two fills that abut carry one edge
/// between them instead of two copies, and every later step — polygonize, coverage union,
/// coverage simplify — is exact rather than approximate.
fn arrangement_faces(members: &[Geometry]) -> Option<Vec<Geom>> {
    let mut lines: Vec<Geometry> = Vec::with_capacity(members.len());
    for m in members {
        lines.push(m.boundary().ok()?);
    }
    let collection = Geometry::create_geometry_collection(lines).ok()?;
    let noded = collection.node().ok()?;
    let polygonized = Geometry::polygonize(&[noded]).ok()?;
    let mut faces = Vec::new();
    collect_polygons(from_geos(&polygonized), &mut faces);
    (!faces.is_empty()).then_some(faces)
}

/// For each face, the index (into `members`) of the fill that would be **visible** there.
/// `member_of[i]` maps that back to a position in `comp`, since the members are the component's
/// *valid* fills only.
///
/// The representative point is a `GEOSPointOnSurface`, which is guaranteed to lie in the face's
/// interior, so "which fills cover this face" is decided by a single point test against
/// prepared geometries — with an `STRtree` to keep the candidate set to the polygons whose box
/// contains the point. Ties do not exist: the winner is the maximum by `(z_index, seq)`, the
/// device's paint order, and a later span paints over an earlier one.
///
/// `None` for a face nothing covers.
fn assign_faces(
    faces: &[Geom],
    members: &[Geometry],
    fills: &[Fill],
    comp: &[usize],
    member_of: &[usize],
) -> Option<Vec<Option<usize>>> {
    let prepared: Vec<PreparedGeometry<'_>> =
        members.iter().map(|m| m.to_prepared_geom()).collect::<Result<_, _>>().ok()?;
    let mut tree = STRtree::<usize>::with_capacity(members.len().max(1)).ok()?;
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
            if !prepared[i].contains_xy(x, y).ok()? {
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
/// returning how many faces changed hands (see the module docs for why this replaces the cull).
///
/// This is the cartographic **eliminate** operator, run to a fixed point rather than in one sweep.
/// Faces are grouped into clusters (each face starts as its own); the smallest cluster still under
/// the threshold is merged into the neighbouring cluster it shares the **longest boundary** with,
/// taking that neighbour's class, and its area is added to it. Repeat. Because a merge grows the
/// survivor, a cluster of specks coalesces outward step by step and the loop ends with every
/// cluster at or above the threshold — which is the property a one-pass version cannot have: there,
/// specks whose longest neighbour is another speck settle on each other and the threshold stops
/// binding altogether.
///
/// Adjacency is read straight off the arrangement rather than recomputed with GEOS. Polygonize
/// emits faces that already share their edges vertex for vertex, so an undirected segment keyed by
/// its two endpoints' exact bit patterns identifies the (at most two) faces on either side of it —
/// `O(total vertices)`, exact, and with no geometry predicate anywhere near it. Shared length sums
/// those segments with longitude foreshortened at the segment's own latitude, so "longest" means
/// longest on the ground.
///
/// Deterministic throughout: the merge order is (area, cluster id) and the target is (shared
/// length, cluster id), both total orders, so the pass cannot depend on hash or thread order.
///
/// Only covered faces take part in either role: a face nobody covers is a genuine gap
/// ([`assign_faces`]), and absorbing one — or into one — would invent fill or destroy it. A cluster
/// with no covered neighbour left simply stays under the threshold.
fn eliminate_small_faces(faces: &[Geom], winners: &mut [Option<usize>], e: Eliminate) -> usize {
    let n = faces.len();
    // --- adjacency: undirected segment -> the face(s) carrying it ---
    //
    // A **sorted list**, not a hash map. A country-scale arrangement carries millions of segments,
    // and a `HashMap<Key, Vec<_>>` over them spends more on its buckets and its per-key `Vec`
    // headers than on the coordinates — enough, measured, to push a whole-extract pack past the
    // 4 GB ceiling on its own. One flat `Vec` sorted in place costs 40 bytes per segment and
    // nothing else, and after the sort the two faces sharing an edge are simply adjacent entries.
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

    // Shared ground length per unordered face pair — both sides covered, or it is not a merge
    // candidate. A run of one is an outer edge; a run of more than two is non-manifold and the
    // arrangement should not produce it, so both are skipped rather than guessed at.
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
        if winners[i].is_none() || winners[j].is_none() {
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

    // --- clusters: a union-find whose representative also carries the class and the running area ---
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
    // and a pop whose area no longer matches the live one is a stale entry and is discarded.
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(OrdF64, usize)>> = faces
        .iter()
        .enumerate()
        .filter(|(i, _)| winners[*i].is_some() && area[*i] < e.min_area_px)
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
        // stable however the map iterated.
        let mut best: Option<(usize, f64)> = None;
        for (&other, &len) in &adj[small] {
            let root = find(&mut parent, other);
            if root == small {
                continue;
            }
            if best.is_none_or(|(bi, bl)| len > bl || (len == bl && root < bi)) {
                best = Some((root, len));
            }
        }
        let Some((into, _)) = best else {
            continue; // an island with nothing covered beside it: it stays as it is
        };
        // `small` joins `into` and takes its class; the survivor keeps its own id so the heap's
        // other entries for it stay meaningful.
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

    // --- every face takes its cluster's owner ---
    let mut moved = 0;
    for fi in 0..n {
        if winners[fi].is_none() {
            continue;
        }
        let root = find(&mut parent, fi);
        if root != fi && winners[root] != winners[fi] {
            winners[fi] = winners[root];
            moved += 1;
        }
    }
    moved
}

/// A total order over the `f64` areas the heap sorts by. Every value here is a finite,
/// non-negative projected area, so `total_cmp` is an ordinary comparison — the wrapper exists
/// only because `f64` is not `Ord`.
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

/// Dissolve one class's faces with `GEOSCoverageUnion` — the cheap union that assumes what is
/// already true here: the faces come out of one arrangement, so their shared edges match
/// vertex for vertex and the union is a boundary walk rather than an overlay. `None` on any
/// GEOS failure — and on a class of one, where there is nothing to dissolve — which leaves the
/// class's faces as they are (still glued, merely more of them).
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
            dashed: false,
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

    /// The shared seam: the wiggly boundary both fills carry, from (1,0) up to (1,1). Two
    /// abutting OSM ways reference the same boundary nodes, so both copies are identical here
    /// too — which is exactly what an independent simplify then fails to keep.
    const SEAM: [(f64, f64); 6] = [(1.12, 0.2), (0.95, 0.35), (1.18, 0.5), (0.9, 0.62), (1.14, 0.8), (1.0, 1.0)];

    /// The western fill: a tall slab whose right edge carries the seam and then runs straight
    /// up to y=10. Its ring is far longer than its neighbour's, which is what makes a
    /// per-feature Douglas–Peucker resolve the shared chain differently on the two sides —
    /// the everyday case of a big landuse block against a small parcel.
    fn seam_west() -> Geom {
        let mut ring = vec![(0.0, 0.0), (1.0, 0.0)];
        ring.extend(SEAM.iter().copied());
        ring.extend([(1.0, 10.0), (0.0, 10.0)]);
        poly(&ring)
    }

    /// The eastern fill: a unit square whose left edge is the same seam, reversed.
    fn seam_east() -> Geom {
        let mut ring = vec![(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
        ring.extend(SEAM.iter().rev().skip(1).copied());
        poly(&ring)
    }

    /// A feature's vertices in the seam band — the two copies of the shared boundary.
    fn seam_verts(g: &Geom) -> Vec<(f64, f64)> {
        verts(g).into_iter().filter(|(x, y)| *x > 0.5 && *x < 1.5 && *y < 1.001).collect()
    }

    /// **The tearing test.** Two abutting fills of *different* classes share a boundary with a
    /// bump on it. Simplified per feature, each copy of that boundary moves on its own and the
    /// backdrop shows through the difference; simplified as a coverage, the shared edge is cut
    /// once and both sides come back with the identical vertex sequence.
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

    /// The same fixture, simplified the old way, really does tear — otherwise the test above
    /// proves nothing. Per-feature `TopologyPreservingSimplifier` keeps the bump on the wider
    /// polygon and drops it from the narrower one, so the two copies of the seam differ.
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

    /// An overlap resolves by the device paint order: the face both fills cover goes to the one
    /// on top (higher `z_index`), and the hidden part of the lower fill is deleted rather than
    /// stored under something opaque.
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

    /// A face no fill covers is a genuine gap and stays one: the pass never invents fill.
    #[test]
    fn an_uncovered_face_is_not_invented() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        // Eight unit squares around an unmapped centre cell.
        let mut feats = Vec::new();
        for gx in 0..3 {
            for gy in 0..3 {
                if gx == 1 && gy == 1 {
                    continue;
                }
                feats.push((1u8, rect(gx as f64, gy as f64, gx as f64 + 1.0, gy as f64 + 1.0)));
            }
        }
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, None);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!(stats.dropped_faces >= 1, "the centre face belongs to nobody: {stats:?}");
        assert!((area(&out) - 8.0).abs() < 1e-9, "eight squares in, eight squares of area out");
        let holes: usize = out
            .iter()
            .map(|(_, g, _)| match g {
                Geom::Polygon { interiors, .. } => interiors.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(holes, 1, "the unmapped centre survives as a hole, not as fill");
    }

    // --- elimination ---------------------------------------------------------------------------

    /// A threshold in the units the pass takes: `px²` at `mpp`. One square degree at the equator is
    /// `(M_PER_DEG / mpp)²` pixels, so this converts an area in square degrees into the
    /// `min_area_px` that sits exactly on it — the tests then ask for a multiple of a known face.
    fn threshold_for(deg2: f64, mpp: f64) -> Eliminate {
        Eliminate { mpp, min_area_px: deg2 * (M_PER_DEG / mpp) * (M_PER_DEG / mpp) }
    }

    /// **The elimination test.** A speck of one class inside a wall of another does not leave a
    /// hole when it is too small to draw — it is absorbed by the neighbour it shares the most
    /// boundary with, and comes back as that class's ground. Area is conserved exactly: this is a
    /// relabelling, not a cull.
    #[test]
    fn a_small_face_is_absorbed_by_its_longest_neighbour() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        // A 1x1 host with a 0.1x0.1 speck of another class biting into its left edge.
        let host = rect(0.0, 0.0, 1.0, 1.0);
        let speck = rect(0.0, 0.4, 0.1, 0.5); // 0.01 deg², on top (z 5) so it wins its own face
        let mpp = 100.0;
        // Threshold above the speck and below the host.
        let e = Some(threshold_for(0.5, mpp));
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, speck)], &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.eliminated, 1, "exactly the speck moved: {stats:?}");
        assert!((area(&out) - 1.0).abs() < 1e-9, "the ground is conserved, not culled: {out:?}");
        assert!(out.iter().all(|(sid, _, _)| *sid == 1), "and all of it belongs to the host class: {out:?}");
    }

    /// Without a threshold nothing moves — the pass is the one it always was, and the parameter is
    /// what turns elimination on.
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

    /// **Longest boundary decides**, not proximity or index order. A sliver wedged between two
    /// classes shares a long edge with one and a short one with the other, and joins the long one.
    #[test]
    fn absorption_follows_the_longest_shared_boundary() {
        let classes = [fill_style(1, 0, 0x0001), fill_style(2, 1, 0x0002), fill_style(3, 5, 0x0003)].map(|s| s);
        let classes = merge_classes(&classes);
        // West block 0..1, east block 1.1..2, and a 0.1-wide sliver between them whose long side
        // (height 1) faces west and whose short side (height 1, but only 0.02 of it) faces east.
        let west = poly(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        let east = poly(&[(1.1, 0.0), (2.0, 0.0), (2.0, 1.0), (1.1, 1.0)]);
        // The sliver touches west along its whole height and east along a stub only.
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

    /// A face nobody covers is still a gap after elimination: it is neither absorbed (that would
    /// invent fill) nor absorbed *into* (that would destroy the gap).
    #[test]
    fn elimination_never_fills_an_uncovered_face() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        let mut feats = Vec::new();
        for gx in 0..3 {
            for gy in 0..3 {
                if gx == 1 && gy == 1 {
                    continue;
                }
                feats.push((1u8, rect(gx as f64, gy as f64, gx as f64 + 1.0, gy as f64 + 1.0)));
            }
        }
        let mpp = 100.0;
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, Some(threshold_for(4.0, mpp)));
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!(stats.dropped_faces >= 1, "the centre still belongs to nobody: {stats:?}");
        assert!((area(&out) - 8.0).abs() < 1e-9, "eight squares in, eight squares of ground out");
        let holes: usize = out
            .iter()
            .map(|(_, g, _)| match g {
                Geom::Polygon { interiors, .. } => interiors.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(holes, 1, "the unmapped centre survives as a hole rather than being invented into fill");
    }

    /// **The fixed point.** A long chain of equal specks, each other's nearest neighbour, has no
    /// face big enough to be a root anywhere in it — a single absorption sweep would pair them off
    /// and stop, leaving the threshold binding nothing. Run to a fixed point they coalesce all the
    /// way into one class instead.
    #[test]
    fn a_chain_of_equal_specks_coalesces_completely() {
        let styles: Vec<Style> = (1..=8).map(|i| fill_style(i, i as i8, 0x0001 + i as u16)).collect();
        let classes = merge_classes(&styles);
        // Eight 1x1 tiles in a row, each its own class, each far below the threshold.
        let feats: Vec<(u8, Geom)> = (0..8).map(|i| (i as u8 + 1, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        let mpp = 100.0;
        // Threshold above the whole row, so nothing can ever satisfy it.
        let e = Some(threshold_for(100.0, mpp));
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!((area(&out) - 8.0).abs() < 1e-9, "all eight tiles of ground survive: {out:?}");
        let live: std::collections::BTreeSet<u8> = out.iter().map(|(sid, _, _)| *sid).collect();
        assert_eq!(live.len(), 1, "the row coalesced into a single class, not four pairs: {live:?}");
        assert_eq!(out.len(), 1, "and into a single polygon: {out:?}");
    }

    /// Determinism with elimination on: the absorption order (ascending area, then face index) and
    /// the neighbour choice (longest edge, then lowest index) are both total, so the parallel pass
    /// cannot leak into the bytes.
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

    /// Geometry GEOS will not touch — a 3-position ring, a self-intersecting bow-tie — **sits the
    /// arrangement out** rather than poisoning it: it comes back byte-for-byte, marked for the
    /// ordinary per-feature path, while its valid neighbours still get the coverage treatment.
    /// Nothing is dropped, which is the one thing a bake may never do.
    #[test]
    fn broken_geometry_sits_out_without_losing_features() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001)]);
        // A ring too short to be a ring, and a ring that crosses itself: GEOS builds the second
        // one happily and then answers questions about it however it likes.
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

    /// A component with nothing usable in it at all falls back wholesale — the never-drop
    /// guarantee at its limit.
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

    /// Outlined polygons (`color2`) and lines never participate: a stroked wall is visible, so
    /// dissolving or re-cutting it would change the picture.
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

    /// Determinism: the same input twice is the same output, coordinate for coordinate — the
    /// pass runs its components in parallel, so this is the assertion that the parallelism
    /// cannot leak into the bytes.
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

    /// The emission device: a class's polygons are emitted at its **first member's** position,
    /// and passthrough features keep their own place around it — the ordering `merge_fills`
    /// established, so switching a tier to this pass does not reshuffle the tree.
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
