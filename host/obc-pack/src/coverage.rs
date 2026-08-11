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
//! # The decimation pre-pass
//!
//! Steps 2–3 above are a planar overlay, and an overlay's cost is driven by the number of
//! *vertices* it has to node, not the number of polygons. Full-detail OSM landcover carries a
//! vertex every few metres — detail a tier whose tolerance is hundreds of metres is about to throw
//! away anyway, but which the arrangement pays for in full first. On the Freiburg extract that was
//! the difference between a pack that fits the host memory law and one that does not.
//!
//! So before anything is noded, each participating fill is pre-simplified **on its own** with the
//! ordinary [`crate::geom::topology_preserve_simplify`] at
//! `tier tolerance / `[`DECIMATE_DIVISOR`], floored at [`DECIMATE_FLOOR_M`] metres and never
//! coarser than the tier's own tolerance. At the coarse tiers this pass runs on that is deeply
//! sub-pixel — the 2200 m tier decimates at 275 m, two thirds of a pixel at the 400 m/px it is
//! first shown at — so it cannot change the picture, while cutting the vertices entering the
//! overlay by about an order of magnitude.
//!
//! It is not a free lunch, and the cost is paid by the elimination step below: because each fill is
//! decimated independently, two neighbours' copies of a shared boundary walk apart by up to the
//! decimation tolerance. Where they overlap, face assignment already resolves it (the visible class
//! wins). Where they part, a **micro-gap** appears — a face nothing covers, sub-pixel wide, which
//! step 3 would drop as backdrop. Healing those is what makes decimation safe, and it is the same
//! operator that already eliminates small *covered* faces.
//!
//! The two are therefore one lever, and the code says so: the pre-pass runs **only** on a tier that
//! has an elimination threshold to heal with. A tier without one gets the full-detail arrangement
//! it has always got, slowly and glued.
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
//! An **uncovered** face is absorbed too — into a *covered* neighbour, never the other way round —
//! but only if it is a **sliver**, and that qualifier is the whole of the rule. Healing exists to
//! close the micro-gaps the decimation pre-pass opens, and the ones OSM ships with (two landcover
//! polygons digitised a few metres apart leave a crack that is nothing but backdrop at any zoom).
//! Both are *thin*: a gap the pre-pass can open is at most two decimation tolerances wide. Merely
//! *small* is a different thing entirely — at the coarse tier the elimination threshold is 40 km²,
//! and a bay, a tarn, a fjord below that is geography, not an artefact. So an uncovered face joins a
//! neighbour only when its mean half-width (area over perimeter) is under
//! [`HEAL_WIDTH_TOLERANCES`] decimation tolerances **and** it is under the tier's threshold — a
//! strict subset of what the covered rule takes. Compact water stays water however small it is, and
//! the direction of the rule keeps the rest honest: absorbing *into* a gap would delete map content,
//! so it never happens.
//!
//! **What the threshold measures is decided by the pre-dissolve** ([`predissolve`]), and that is
//! the one place these cost levers change the picture rather than just the bill. Fragmented
//! same-class landcover has to survive elimination at its **true contiguous size**, not parcel by
//! parcel. Without the dissolve a class arrives as its individual parcels, so a plain of fragmented
//! farmland is a plain of faces that are each under the threshold, and the fixed point walks them
//! one by one into whatever is around them — on the Rhine valley, into the `natural.land` base
//! underneath, until the far-zoom tier shows bare ground where every finer tier shows farmland.
//! With it, contiguous farmland is one face of its real size and it stays. Elimination is supposed
//! to drop what is too small **to see**, and only a dissolved class states that size honestly.
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
    try_polygon_to_geos, union_polygons, Bounds, Geom,
};
use crate::merge::ClassKey;
use crate::progress::Progress;

/// What the pass did to one LOD, for the per-tier log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CoverageStats {
    /// Participating fill polygons consumed.
    pub inputs: usize,
    /// What the per-class pre-dissolve left of them — the polygons the arrangement is built over.
    pub dissolved: usize,
    /// Polygons emitted in their place.
    pub outputs: usize,
    /// Ring positions those fills arrived with.
    pub vertices_in: usize,
    /// Ring positions that actually entered the arrangement, after the decimation pre-pass.
    pub vertices_arranged: usize,
    /// Bbox-connected components the arrangement was built over.
    pub components: usize,
    /// Faces the arrangements produced.
    pub faces: usize,
    /// Faces no fill covered and no covered neighbour absorbed — genuine gaps, left as backdrop
    /// rather than invented into fill.
    pub dropped_faces: usize,
    /// Faces below the tier's threshold that were absorbed into a neighbour (see the module docs).
    pub eliminated: usize,
    /// Uncovered faces below the tier's threshold that a covered neighbour absorbed — the
    /// micro-gaps decimation opens, plus the ones the source data already had.
    pub healed: usize,
    /// Class groups whose `GEOSCoverageUnion` refused, leaving that class's faces undissolved.
    pub dissolve_failures: usize,
    /// Components that hit a GEOS failure and fell back to the per-feature path.
    pub fallbacks: usize,
}

/// GEOS `STRtree` **node capacity**: the number of children a tree node may hold, and *not* a
/// count of items to reserve room for.
///
/// [`geos::STRtree::with_capacity`] passes this value straight to `GEOSSTRtree_create`, so the
/// natural reading of the name — "how many things am I about to insert" — builds a tree of a single
/// flat node, and every query then scans every envelope in it. That is not a slow index, it is no
/// index: on this pass' quarter-million fills against half a million faces it was **minutes** of
/// linear search per tier, and it was the whole of the pass' cost. 10 is GEOS' own documented
/// default.
const STRTREE_NODE_CAPACITY: usize = 10;

/// A member with more coordinates than this gets a `PreparedGeometry` for the face assignment;
/// smaller ones are point-tested directly. See [`assign_faces`].
const PREPARE_ABOVE_COORDS: usize = 64;

/// How many decimation tolerances of **mean half-width** an uncovered face may have and still be
/// healed into a covered neighbour (see the module docs and [`sliver_half_width_m`]).
///
/// The bound comes from what the pre-pass can actually do. Two neighbours' copies of a shared
/// boundary are decimated independently, so each may move a full tolerance, in opposite directions:
/// the widest gap it can open is `2 × dec_tol`, and a ribbon of width `w` has mean half-width `w/2`,
/// so `dec_tol` is the worst case. Two doubles it, which covers a sliver that is fatter at a
/// junction than along its length while staying far away from anything with geography in it — a
/// compact shape's mean half-width grows with its size (a disc's is `r/2`, a square's `s/4`), so a
/// bay wide enough to be a bay fails this test long before its *area* would have saved it.
const HEAL_WIDTH_TOLERANCES: f64 = 2.0;

/// The decimation pre-pass runs at the tier's tolerance divided by this — small enough to be
/// deeply sub-pixel at the scale the tier is drawn at, large enough to take an order of magnitude
/// of vertices out of the arrangement. See the module docs.
const DECIMATE_DIVISOR: f64 = 8.0;

/// Floor on the decimation tolerance, metres: below this the vertices removed stop paying for the
/// simplify that removes them. A tier whose own tolerance is finer than this decimates at its own
/// tolerance instead — the pre-pass is never allowed to be coarser than the pass it feeds.
const DECIMATE_FLOOR_M: f64 = 10.0;

/// The decimation tolerance for a tier simplifying at `tol` degrees, `0.0` for "do not decimate".
///
/// A tier that asked for no simplify at all (`tol == 0.0`, dissolve and re-cut only) gets no
/// decimation either: it asked for its geometry back unmoved, and the pre-pass would move it.
fn decimation_tol(tol: f64) -> f64 {
    if tol <= 0.0 {
        return 0.0;
    }
    (tol / DECIMATE_DIVISOR).max(DECIMATE_FLOOR_M / M_PER_DEG).min(tol)
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

/// What the decimation pre-pass decided about one fill, and the geometry the arrangement should
/// build from.
enum Prep {
    /// GEOS will not touch it (unconvertible, or invalid): it sits the arrangement out and passes
    /// through untouched — see [`coverage_component`].
    SitOut,
    /// Usable exactly as it arrived: the tier asked for no simplify, or the decimation did not
    /// produce something usable and the original is the honest input.
    AsIs,
    /// Usable, decimated to [`decimation_tol`].
    Decimated(Geom),
}

/// A face's **mean half-width** on the ground, in metres: its area divided by its perimeter.
///
/// That ratio is the shape test healing needs and an area test cannot give. For a ribbon of width
/// `w` it is `w/2` however long the ribbon runs; for a disc of radius `r` it is `r/2`; for a square
/// of side `s`, `s/4`. So it separates "thin" from "small": a hundred-metre crack a kilometre long
/// and a compact kilometre-wide bay have similar areas and utterly different answers here.
///
/// Both quantities are measured in degrees with longitude foreshortened at the face's own mean
/// latitude, then scaled by `M_PER_DEG` — the same metric [`eliminate_small_faces`] measures shared
/// edges with. One cosine for the whole face is exact enough: a face this test can pass is, by
/// construction, small. Holes count against the area and towards the perimeter, which is the honest
/// reading (a ring-shaped gap is thin). A non-polygon or a degenerate ring answers infinity, so it
/// is never healed.
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

/// Ring positions in a polygon — the unit the overlay's cost is measured in.
fn vertex_count(g: &Geom) -> usize {
    match g {
        Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
        Geom::Line(c) => c.len(),
        Geom::Multi(parts) => parts.iter().map(vertex_count).sum(),
        Geom::Empty => 0,
    }
}

/// The identity of a tier's participating fill set, so [`PredissolveCache`] can tell whether the
/// dissolve it is holding was computed from the same thing.
///
/// Two parts, and both must match. `composition` is every fill's `(seq, style_id)` in order,
/// compared exactly: it is the set as the tier presented it, so a preset whose two coverage tiers
/// admit different features — a different `min_lod` cut, a line merge that lands differently —
/// gives a different list and misses the cache. `geometry` is a hash over every coordinate, the
/// guard for the one thing the composition cannot see: that the shapes behind those seqs are the
/// shapes the cached dissolve was built from. A miss only costs the work again, so this errs
/// towards missing.
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
            let Geom::Polygon { exterior, interiors } = &f.geom else { continue };
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

/// A memo for [`predissolve`], shared by the coverage tiers of one build.
///
/// Every coverage tier dissolves the same classes over (usually) the same fills — on the shipped
/// preset both far-zoom tiers take the identical 237 196 polygons down to the identical 89 512 —
/// and that dissolve is a parallel GEOS union over the whole extract, which is a large part of the
/// pass' allocator churn. Only the *decimation* below it is per tier, because only the tolerance
/// differs. So the undecimated dissolve is computed once and shared.
///
/// It holds an `Arc` rather than handing out clones: the arrangement only ever reads the fills.
/// [`PredissolveCache::clear`] drops it, and the caller is expected to call that once the last
/// coverage tier is behind it — the fine tiers are where the pack's peak lives, and they have no
/// use for a hundred megabytes of dissolved coarse-tier geometry.
#[derive(Default)]
pub struct PredissolveCache {
    entry: std::sync::Mutex<Option<(FillSetId, std::sync::Arc<Vec<Fill>>)>>,
}

impl PredissolveCache {
    /// An empty cache. One per build; sharing it across builds would be sound but pointless.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget what is held. Cheap, and idempotent.
    pub fn clear(&self) {
        if let Ok(mut slot) = self.entry.lock() {
            *slot = None;
        }
    }

    /// The dissolve of `fills` — from the memo if it was computed from exactly this set, otherwise
    /// computed now and memoised. A poisoned lock falls through to computing it, since the cache is
    /// an optimisation and never the source of truth.
    fn dissolve(&self, fills: Vec<Fill>) -> std::sync::Arc<Vec<Fill>> {
        let id = FillSetId::of(&fills);
        let Ok(mut slot) = self.entry.lock() else { return std::sync::Arc::new(predissolve(fills)) };
        if let Some((cached, dissolved)) = slot.as_ref() {
            if *cached == id {
                return std::sync::Arc::clone(dissolved);
            }
        }
        // Drop whatever else was held *before* building the replacement, so the two never coexist.
        *slot = None;
        let dissolved = std::sync::Arc::new(predissolve(fills));
        *slot = Some((id, std::sync::Arc::clone(&dissolved)));
        dissolved
    }
}

/// Dissolve each class's fills into their union **before** the arrangement is built.
///
/// This is [`crate::merge::merge_fills`]' operator, run here for a different reason. There it saves
/// spans on the device; here it deletes work: a shared boundary between two *same-class* parcels is
/// invisible in the output either way, but the arrangement still has to node it, polygonize a face
/// on each side of it, index both, assign both and dissolve them back together at the end. Rural
/// OSM is wall-to-wall parcels of a handful of classes, so most boundaries in the extract are of
/// exactly that kind — on Freiburg it is 237 000 fills down to 90 000. Deleting them early is
/// strictly less to node, fewer faces to hold, fewer prepared geometries to index, and it comes
/// before the decimation, so the boundaries that *do* survive are decimated once between two
/// classes rather than once per parcel.
///
/// The union is [`crate::geom::union_polygons`], which clusters by shared vertices and unions each
/// cluster on one thread; a cluster GEOS refuses passes its own polygons through unmerged, so
/// nothing is dropped and an invalid parcel still reaches [`prepare_fills`] to sit the arrangement
/// out. Order is deterministic (classes in canonical-id order, parts as `union_polygons` emits
/// them).
///
/// **Paint order becomes per class.** A dissolved polygon carries the whole class's *first* `seq`,
/// because that is where [`coverage_simplify_fills_with`]'s `Slot::Group` emits the class — after
/// this pass a class is one block of records, and the device paints the block at that position, so
/// a per-parcel `seq` would be answering a question the output no longer asks. It changes nothing
/// unless two *different* classes share a `z_index`, which is the only case `seq` ever decided.
fn predissolve(fills: Vec<Fill>) -> Vec<Fill> {
    // Classes in canonical-id order; members in input order within each.
    let mut groups: BTreeMap<u8, Vec<Fill>> = BTreeMap::new();
    for f in fills {
        groups.entry(f.canonical).or_default().push(f);
    }
    let mut out: Vec<Fill> = Vec::new();
    for (canonical, members) in groups {
        if members.len() < 2 {
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
            // The whole class refused: keep it exactly as it arrived (this is a cost optimisation,
            // never a correctness step).
            None => {
                drop(refs);
                out.extend(members);
            }
        }
    }
    out
}

/// The decimation pre-pass: validate every fill and pre-simplify it at `dec_tol` (see the module
/// docs). Runs in parallel over the fills — each task builds, simplifies and reads back its GEOS
/// geometry on its own thread, so nothing `!Send` crosses a boundary — which also moves the
/// per-fill validity check off the single rayon task that owns the one big component.
///
/// Anything GEOS refuses, at either step, degrades rather than fails: an invalid input sits the
/// arrangement out (unchanged, as before this pass existed), and a decimation that errors or comes
/// back invalid or empty leaves the fill at full detail. The arrangement is therefore fed exactly
/// the same *set* of fills as it was before, only lighter.
fn prepare_fills(fills: &[Fill], dec_tol: f64) -> Vec<Prep> {
    fills
        .par_iter()
        .map(|f| {
            let Some(g) = try_polygon_to_geos(&f.geom) else { return Prep::SitOut };
            if !g.is_valid().unwrap_or(false) {
                return Prep::SitOut;
            }
            if dec_tol <= 0.0 {
                return Prep::AsIs;
            }
            let Ok(s) = g.topology_preserve_simplify(dec_tol) else { return Prep::AsIs };
            // A simplify that broke validity is not an input this pass may node: the arrangement
            // assumes valid members (see `coverage_component`), so the full-detail original stands.
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
    coverage_simplify_fills_with(features, classes, tol, eliminate, &PredissolveCache::new(), &Progress::silent())
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
    cache: &PredissolveCache,
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
    stats.vertices_in = fills.iter().map(|f| vertex_count(&f.geom)).sum();
    if fills.is_empty() {
        // Nothing participated (a lines-only tier, or one whose polygons are all outlined): the
        // input echoes back untouched, and there are no `Group` slots to fill.
        return (slots.into_iter().map(|s| s.into_pass()).collect(), stats);
    }

    // --- Phase 2: dissolve each class, decimate what is left, then take bbox-connected components
    // (see the module docs). Decimation only shrinks a polygon's bounds (it keeps a subset of its
    // vertices), so the components computed from the post-dissolve bounds stay the conservative
    // superset they have to be.
    //
    // Decimation is gated on the tier having an elimination threshold, because that threshold is
    // what closes the micro-gaps decimating independently opens: without it the pre-pass would
    // trade the tear this whole module exists to remove for a cheaper arrangement, which is no
    // trade at all. The two levers are one lever. ---
    let fills = cache.dissolve(fills);
    stats.dissolved = fills.len();
    let dec_tol = if eliminate.is_some() { decimation_tol(tol) } else { 0.0 };
    // The sliver bound healing measures uncovered faces against, in metres. It is derived from the
    // decimation tolerance, so a tier that does not decimate opens no gaps and heals none.
    let heal_half_width_m = HEAL_WIDTH_TOLERANCES * dec_tol * M_PER_DEG;
    let preps = prepare_fills(&fills, dec_tol);
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

    // --- Phase 3: one arrangement per component, in parallel. Every GEOS object a task
    // touches is built, used and dropped on that task's own thread (`geos::Geometry` is
    // `!Send`); only plain `Geom` crosses a thread boundary. ---
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

    // --- Phase 4: emit in slot order, each class's polygons at its first member's position. ---
    let mut by_class: HashMap<u8, Vec<(u8, Geom, bool)>> = HashMap::new();
    for (comp, result) in components.iter().zip(results) {
        match result {
            Some(out) => {
                stats.faces += out.faces;
                stats.dropped_faces += out.dropped_faces;
                stats.eliminated += out.eliminated;
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
    healed: usize,
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
///
/// `preps` is the decimation pre-pass' verdict per fill, indexed like `fills`; it is what decides
/// which members sit out, and supplies the (lighter) geometry the arrangement is built from.
fn coverage_component(
    fills: &[Fill],
    preps: &[Prep],
    comp: &[usize],
    tol: f64,
    eliminate: Option<Eliminate>,
    heal_half_width_m: f64,
) -> Option<ComponentOut> {
    // The members as GEOS polygons — also the inputs of the point-in-polygon assignment.
    let mut members: Vec<Geometry> = Vec::with_capacity(comp.len());
    let mut member_of: Vec<usize> = Vec::with_capacity(comp.len());
    // The `Geom` each member was built from, for the single-member shortcut below.
    let mut prepared: Vec<&Geom> = Vec::with_capacity(comp.len());
    let mut sat_out: Vec<(u8, Geom, bool)> = Vec::new();
    for (k, &i) in comp.iter().enumerate() {
        // The pre-pass already validated (and possibly decimated) this fill; `SitOut` is its way
        // of saying GEOS would not have it, which is the case the arrangement must not see.
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
            (vec![prepared[0].clone()], vec![Some(0usize)])
        } else {
            let faces = arrangement_faces(&members)?;
            let winners = assign_faces(&faces, &members, fills, comp, &member_of)?;
            (faces, winners)
        }
    };
    let n_faces = faces.len();

    // --- Elimination: a face under the tier's threshold joins the neighbour it shares the most
    // boundary with, so the dissolve below absorbs it instead of the cull deleting it. An
    // *uncovered* face under the threshold joins a covered neighbour the same way, which is what
    // closes the micro-gaps decimation opens (see the module docs). ---
    let (eliminated, healed) = match eliminate {
        Some(e) => eliminate_small_faces(&faces, &mut winners, e, heal_half_width_m),
        None => (0, 0),
    };
    // After elimination, because healing is exactly the operation that turns an uncovered face
    // into a covered one: what is still uncovered here is what stays backdrop.
    let dropped = winners.iter().filter(|w| w.is_none()).count();

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
            healed,
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
    Some(ComponentOut { polys, faces: n_faces, dropped_faces: dropped, eliminated, healed, dissolve_failures })
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
    // Each of these steps copies every coordinate again, so each input is freed the moment its
    // successor exists rather than at the end of the function: on a country-scale arrangement a
    // spare copy of every boundary is hundreds of megabytes, and the whole point of this pass'
    // recent shape is that it fits the host memory law. (`noded` needs no `drop`: it is moved into
    // the temporary array `polygonize` borrows, which dies with the statement.)
    let noded = {
        let collection = Geometry::create_geometry_collection(lines).ok()?;
        collection.node().ok()?
    };
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
/// interior, so "which fills cover this face" is decided by a single point test against each
/// candidate member — with an `STRtree` to keep the candidate set to the polygons whose box
/// contains the point. Ties do not exist: the winner is the maximum by `(z_index, seq)`, the
/// device's paint order, and a later span paints over an earlier one.
///
/// **Only the big members are prepared.** A `PreparedGeometry` earns its index when the same shape
/// is queried many times; after the pre-dissolve and the decimation the typical member is a handful
/// of vertices tested two or three times, and building an indexed point locator for each of ninety
/// thousand of those costs far more — in the arena churn this pass is memory-budgeted on, most of
/// all — than the ray casts it saves. The few genuinely large members (the land polygon under
/// everything, a big forest) are the opposite case, and every face's point falls inside their
/// envelope, so they are prepared and the rest are tested directly. Same predicate either way.
///
/// `None` for a face nothing covers.
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
/// returning `(covered faces that changed class, uncovered faces a neighbour healed)` — see the
/// module docs for why this replaces the cull.
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
/// **Uncovered faces play one role only, and only if they are slivers.** An uncovered face
/// ([`assign_faces`] found nothing covering it) may be absorbed *into* a covered neighbour when its
/// [`sliver_half_width_m`] is under `heal_half_width_m` — that is the healing the module docs
/// describe, and it is how the micro-gaps decimation opens are closed. Never the reverse: the target
/// of every absorption is a covered cluster, so a covered face can never be swallowed by a gap and
/// lose its fill. An uncovered cluster therefore never grows, and a gap that is compact rather than
/// thin, or at or above the threshold, or with no covered neighbour at all, is left exactly as it
/// is. `heal_half_width_m <= 0.0` turns healing off entirely.
fn eliminate_small_faces(
    faces: &[Geom],
    winners: &mut [Option<usize>],
    e: Eliminate,
    heal_half_width_m: f64,
) -> (usize, usize) {
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

    // Shared ground length per unordered face pair — at least one side covered, or there is no
    // absorption either way (a gap cannot be a target, and gap-into-gap would only move a gap
    // around). A run of one is an outer edge; a run of more than two is non-manifold and the
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
    // Uncovered faces are seeded too, but only the **thin** ones: healing exists for the cracks
    // decimation opens and OSM ships with, and `sliver_half_width_m` is what tells those apart from
    // a small piece of geography. The area threshold still applies on top, so a healed face is
    // always a subset of what a covered face at the same size would be.
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
        // stable however the map iterated. Only a **covered** cluster may be the target: absorbing
        // into a gap would delete fill rather than tidy it away.
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
    //
    // A cluster's owner is its root's, and a root is always covered by the time anything joins it
    // (the target rule above), so a face can only gain fill here, never lose it.
    let (mut moved, mut healed) = (0, 0);
    for fi in 0..n {
        let root = find(&mut parent, fi);
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
    (moved, healed)
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

    /// Eight unit squares of one class around an unmapped centre cell, plus a neighbour of a second
    /// class abutting the block's right edge. The ring dissolves into one polygon whose hole is the
    /// unmapped cell; the neighbour is what makes the component a real arrangement rather than the
    /// single-member shortcut, so the hole reaches [`assign_faces`] as a face nothing covers.
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

    /// Every ring of a feature set, counted — the unmapped centre must survive as one of them.
    fn hole_count(out: &[(u8, Geom, bool)]) -> usize {
        out.iter()
            .map(|(_, g, _)| match g {
                Geom::Polygon { interiors, .. } => interiors.len(),
                _ => 0,
            })
            .sum()
    }

    /// A face no fill covers is a genuine gap and stays one: the pass never invents fill.
    #[test]
    fn an_uncovered_face_is_not_invented() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let (out, stats) = coverage_simplify_fills(ring_around_a_hole(), &classes, 0.0, None);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert!(stats.dropped_faces >= 1, "the centre face belongs to nobody: {stats:?}");
        assert!((area(&out) - 9.0).abs() < 1e-9, "nine squares in, nine squares of area out");
        assert_eq!(hole_count(&out), 1, "the unmapped centre survives as a hole, not as fill");
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

    /// A face nobody covers and that is **too big to be a micro-gap** stays a gap through
    /// elimination: it is neither absorbed *into* (that would destroy the gap) nor healed. The
    /// unit square in the middle here is twice the threshold.
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

    // --- healing -------------------------------------------------------------------------------

    /// A 10x10 fill with an unmapped `w` x `h` hole at its centre, plus a neighbour of a second
    /// class so the component is a real arrangement rather than the single-member shortcut.
    /// Polygonize turns the hole into its own face, which nothing covers — the shape of every
    /// micro-gap decimation can open, with its aspect ratio under the test's control.
    fn host_with_hole(w: f64, h: f64) -> Vec<(u8, Geom)> {
        let (x0, x1) = (5.0 - w * 0.5, 5.0 + w * 0.5);
        let (y0, y1) = (5.0 - h * 0.5, 5.0 + h * 0.5);
        let host = Geom::Polygon {
            exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]],
        };
        vec![(1u8, host), (2u8, rect(10.0, 0.0, 11.0, 1.0))]
    }

    /// The tier the healing tests run at: `tol` 0.08° decimates at 0.01° (an eighth of it), so the
    /// sliver bound is `HEAL_WIDTH_TOLERANCES` x 0.01° of mean half-width. A hole 0.03° thick is
    /// comfortably under that and comfortably over the decimation tolerance itself, so the fixture
    /// exercises the test rather than the pre-pass.
    const HEAL_TOL: f64 = 0.08;

    /// **The healing test.** A decimation-scale crack — thin, and under the tier's threshold — is
    /// absorbed by the covered face around it, so the tier renders complete ground instead of a
    /// hairline of backdrop.
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

    /// **The coastal test, and the reason the rule is about width and not size.** A compact gap of
    /// the *same area* as the sliver above — a bay, a tarn, an unmapped basin — is left alone even
    /// though it sits far below the elimination threshold. Nothing about being small makes a piece
    /// of water into an artefact.
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

    /// **The area cap still binds.** Thin is necessary, not sufficient: a sliver whose area is over
    /// the tier's elimination threshold is left alone, so healing can never take more ground than
    /// the covered rule would at the same size.
    #[test]
    fn a_sliver_over_the_area_threshold_is_not_healed() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        // The same 0.12°² crack, with the threshold moved below it.
        let e = Some(threshold_for(0.05, 100.0));
        let (_out, stats) = coverage_simplify_fills(host_with_hole(4.0, 0.03), &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "over the threshold, however thin: {stats:?}");
        assert_eq!(stats.dropped_faces, 1, "{stats:?}");
    }

    /// The measure itself, on shapes whose answer is arithmetic: a ribbon reports half its width
    /// however long it runs, a square a quarter of its side. That is the whole reason healing tests
    /// this and not area.
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

    /// Healing never runs the other way: a **covered** face under the threshold whose only
    /// neighbour is a gap keeps its fill rather than being swallowed by it. With healing switched
    /// on and the gap far too fat to qualify, this is the rule that makes the pass safe to point at
    /// a real coastline.
    #[test]
    fn a_gap_never_absorbs_a_covered_face() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        // A host with a 6x6 hole, and a 1x1 speck of another class sitting inside that hole so its
        // only neighbour is the uncovered rest of the hole.
        let host = Geom::Polygon {
            exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![vec![(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0), (2.0, 2.0)]],
        };
        let speck = rect(4.0, 4.0, 5.0, 5.0); // 1 deg², wholly inside the hole
                                              // 2 deg²: over the speck, under the 35 deg² hole and the 64 deg² ring around it.
        let e = Some(threshold_for(2.0, 100.0));
        let (out, stats) = coverage_simplify_fills(vec![(1, host), (2, speck)], &classes, HEAL_TOL, e);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        assert_eq!(stats.healed, 0, "the hole is nowhere near thin: {stats:?}");
        let by = |sid: u8| area(&out.iter().filter(|(s, _, _)| *s == sid).cloned().collect::<Vec<_>>());
        assert!(by(2) > 0.5, "the speck kept its fill instead of being eaten by the gap: {out:?}");
    }

    // --- pre-dissolve --------------------------------------------------------------------------

    /// **The pre-dissolve.** A row of same-class parcels reaches the arrangement as one polygon, so
    /// the boundaries between them are never noded, never become two faces, and never have to be
    /// dissolved back together — and the ground is untouched, because a class's union is what the
    /// pass emits for it anyway.
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

    /// A class of one is not run through GEOS at all, and neither is a tier whose fills are all in
    /// different classes — the pre-dissolve is a cost optimisation and must cost nothing when there
    /// is nothing to dissolve.
    #[test]
    fn the_pre_dissolve_leaves_single_member_classes_alone() {
        let styles: Vec<Style> = (1..=3).map(|i| fill_style(i, i as i8, 0x0001 + i as u16)).collect();
        let classes = merge_classes(&styles);
        let feats: Vec<(u8, Geom)> = (0..3).map(|i| (i as u8 + 1, rect(i as f64, 0.0, i as f64 + 1.0, 1.0))).collect();
        let (out, stats) = coverage_simplify_fills(feats, &classes, 0.0, None);
        assert_eq!((stats.inputs, stats.dissolved), (3, 3), "nothing to dissolve: {stats:?}");
        assert!((area(&out) - 3.0).abs() < 1e-9);
    }

    // --- the pre-dissolve cache -----------------------------------------------------------------

    /// One `Fill`, for the identity tests.
    fn a_fill(seq: usize, style_id: u8, g: Geom) -> Fill {
        let bounds = g.bounds();
        Fill { seq, style_id, canonical: style_id, key: (0, 0, 0), geom: g, bounds }
    }

    /// The cache key sees **both** halves of what it claims to identify. Composition alone would
    /// miss a preset that fed different shapes under the same seqs; geometry alone would miss two
    /// tiers whose fills differ only in which style ids are present.
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

    /// **The cache may never answer for a set it did not dissolve.** A second tier with a different
    /// fill set — the `min_lod` cut that admits one more feature — must get its own dissolve, and
    /// the proof is that it matches what a cold cache produces for it.
    #[test]
    fn the_predissolve_cache_misses_on_a_different_fill_set() {
        let classes = merge_classes(&[fill_style(1, 0, 0x0001), fill_style(2, 5, 0x0002)]);
        let coarse = || vec![(1u8, rect(0.0, 0.0, 1.0, 1.0)), (1u8, rect(1.0, 0.0, 2.0, 1.0))];
        // The finer tier admits one more feature, so its set is not the coarse one.
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

    /// And when the set *is* the same — the ordinary case, two coverage tiers over one extract —
    /// the cache changes nothing about the answer. It is a memo, not a mode.
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
        // Two different tolerances over the same fills: the dissolve is shared, the decimation and
        // the simplify are not.
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

    // --- decimation ----------------------------------------------------------------------------

    /// The decimation tolerance: the tier's own over [`DECIMATE_DIVISOR`], floored at
    /// [`DECIMATE_FLOOR_M`] metres, never coarser than the tier itself, and off entirely for a tier
    /// that asked for no simplify.
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

    /// **The decimation pre-pass.** A fill whose boundary carries far more detail than the tier can
    /// show enters the arrangement with an order of magnitude fewer vertices — which is the whole
    /// memory and time lever — while the ground it covers is unchanged to well within the tier's
    /// own tolerance.
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
        // A threshold well under either slab, so nothing real is eliminated — it is here because it
        // is what unlocks the pre-pass (and heals anything the sawtooth's two copies leave behind).
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

    /// Decimation cannot un-glue what the pass exists to glue: it runs *before* the arrangement, so
    /// the shared boundary is still noded once and both sides still come back identical.
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
