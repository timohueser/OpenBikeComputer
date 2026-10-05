//! The cutter's whole-extract feature preparation and per-cell geometry.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use obc_map_scene::M_PER_DEG;

use crate::config::Config;
use crate::geom::{clip_to_box, footprint_below, strip_small_holes, topology_preserve_simplify, Bounds, Geom};
use crate::grid::{cells_intersecting, CellId, UBox};
use crate::ingest::{IngestFeature, Ingested};
use crate::merge::{merge_classes, merge_fills_with, merge_line_classes, merge_line_trails_with, merge_lines_with};
use crate::progress::Progress;
use crate::quadtree::build_lod_with;
use crate::semantic::{SemanticClass, SemanticScheme};
use crate::serialize::Node;

type LodFeatures<'a> = Arc<Vec<(u8, Cow<'a, Geom>)>>;
type MergeKey = (usize, bool, bool);
type MergeMemo<'a> = HashMap<MergeKey, LodFeatures<'a>>;

/// One ladder level: shared merged features, a simplify memo, and a cell candidate index.
pub(super) struct LodSet<'a> {
    pub(super) lod: usize,
    feats: LodFeatures<'a>,
    /// A feature with one candidate cell already gets one simplify pass per LOD.
    simplified: Vec<Option<Box<OnceLock<Geom>>>>,
    /// Parallel to `feats`: the semantic coverage already simplified this feature, so
    /// [`LodSet::cell_tree`] must not simplify it again (that would move the shared boundaries
    /// the coverage glued). All `false` unless the tier is semantic.
    presimplified: Vec<bool>,
    /// `(i, j)` → indices into `feats`. Membership is decided on **inclusive** bounds, so a feature
    /// reaching a seam line is a candidate on both sides and the two cells clip identical geometry.
    buckets: HashMap<(i64, i64), Vec<u32>>,
    /// Simplify tolerance, degrees (`0.0` ⇒ none).
    tol: f64,
    /// Line-only simplify tolerance, degrees (`0.0` ⇒ none).
    line_tol: f64,
    /// The m/px the footprint cull measures at, `None` ⇒ no cull for this level.
    cull_mpp: Option<f64>,
    min_area_px: f64,
    cell_log2: u32,
}

pub(super) struct PreparedSemantic<'a, 's> {
    pub(super) features: Option<&'a [(u8, Geom)]>,
    pub(super) scheme: &'s SemanticScheme,
}

fn lod_includes(feature: &IngestFeature, lod: usize, semantic: bool, scheme: &SemanticScheme) -> bool {
    feature.min_lod <= lod
        && !feature.geom.is_empty()
        && !(semantic
            && matches!(feature.geom, Geom::Polygon { .. } | Geom::Multi(_))
            && matches!(
                scheme.class_of(feature.style_id),
                Some(
                    SemanticClass::Farmland
                        | SemanticClass::Grass
                        | SemanticClass::Forest
                        | SemanticClass::Urban
                        | SemanticClass::Rock
                        | SemanticClass::Ice
                        | SemanticClass::Water
                )
            ))
}

pub(super) fn merge_key(ing: &Ingested, config: &Config, lod: usize, scheme: &SemanticScheme) -> MergeKey {
    let level = &config.lods[lod];
    let count = ing.features.iter().filter(|f| lod_includes(f, lod, level.semantic_coverage, scheme)).count();
    (count, level.semantic_coverage, level.merge_line_trails)
}

/// Build a level's feature set exactly as [`crate::pipeline`] does — `min_lod` filter, then the
/// optional fill-dissolve and line-stitch passes, plus a semantic tier's prebuilt coverage — and
/// index it by cell.
///
/// The merge memo shares identical input sets until their last band. The `min_lod` filters select nested sets,
/// so their count identifies the set for a given semantic filter and line merge mode. Each level
/// memoizes simplification when a cell first uses a feature. Semantic coverage is already simplified
/// globally, so `presimplified` exempts it from that pass.
pub(super) fn prepare_lod<'a>(
    ing: &'a Ingested,
    config: &Config,
    lod: usize,
    cell_log2: u32,
    semantic: PreparedSemantic<'a, '_>,
    merged_sets: &mut MergeMemo<'a>,
    progress: &Progress,
) -> LodSet<'a> {
    let l = &config.lods[lod];
    // `Geom::bounds` panics on an empty geometry, and a merge pass can hand one back, so empties are
    // dropped here — exactly where `build_lod_with` drops them on the whole-extract path.
    let mut feats: Vec<(u8, Cow<'a, Geom>)> = ing
        .features
        .iter()
        .filter(|f| lod_includes(f, lod, l.semantic_coverage, semantic.scheme))
        .map(|f| (f.style_id, Cow::Borrowed(&f.geom)))
        .collect();
    let tol = if l.simplify_m > 0.0 { l.simplify_m / M_PER_DEG } else { 0.0 };
    let line_tol = if l.line_simplify_m > 0.0 { l.line_simplify_m / M_PER_DEG } else { 0.0 };
    // A semantic tier skips `merge_fills`, exactly as [`crate::pipeline`] does.
    let want_merge_fills = config.merge_fills && !l.semantic_coverage;
    let key = (feats.len(), l.semantic_coverage, l.merge_line_trails);
    let mut feats = if let Some(cached) = merged_sets.get(&key) {
        Arc::clone(cached)
    } else {
        if want_merge_fills || config.merge_lines {
            let styles = config.styles();
            let mut owned: Vec<(u8, Geom)> = feats.into_iter().map(|(s, g)| (s, g.into_owned())).collect();
            if want_merge_fills {
                let (merged, m) = merge_fills_with(owned, &merge_classes(&styles), progress);
                crate::pipeline::report_merge(progress, m, "fill polygon", "into");
                owned = merged;
            }
            if config.merge_lines {
                let line_classes = merge_line_classes(&styles);
                let (merged, m) = if l.merge_line_trails {
                    merge_line_trails_with(owned, &line_classes, progress)
                } else {
                    merge_lines_with(owned, &line_classes, progress)
                };
                crate::pipeline::report_merge(progress, m, "line fragment", "into");
                owned = merged;
            }
            feats = owned.into_iter().filter(|(_, g)| !g.is_empty()).map(|(s, g)| (s, Cow::Owned(g))).collect();
        }
        // Keep the land base through merging, then omit it: the renderer clears to land.
        if let Some(land_id) = config.implicit_land_style_id() {
            feats.retain(|(style_id, _)| *style_id != land_id);
        }
        let feats = Arc::new(feats);
        merged_sets.insert(key, Arc::clone(&feats));
        feats
    };
    let mut presimplified = vec![false; feats.len()];

    if l.semantic_coverage {
        let semantic_features = semantic.features.expect("every configured semantic rung is prebuilt");
        let feats = Arc::make_mut(&mut feats);
        feats.reserve(semantic_features.len());
        presimplified.reserve(semantic_features.len());
        for (style_id, geom) in semantic_features {
            if !geom.is_empty() && Some(*style_id) != config.implicit_land_style_id() {
                feats.push((*style_id, Cow::Borrowed(geom)));
                presimplified.push(true);
            }
        }
    }

    let bounds: Vec<Bounds> = feats.iter().map(|(_, g)| g.bounds()).collect();
    let mut buckets: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    let mut simplified = Vec::with_capacity(feats.len());
    for (k, b) in bounds.iter().enumerate() {
        let cells = cells_intersecting(cell_log2, bounds_to_udeg(*b));
        let tolerance = if feats[k].1.is_lineal() { line_tol } else { tol };
        let cache = (cells.len() > 1 && tolerance > 0.0 && !presimplified[k]).then(|| Box::new(OnceLock::new()));
        simplified.push(cache);
        for cell in cells {
            buckets.entry((cell.i, cell.j)).or_default().push(k as u32);
        }
    }
    // The cull's reference scale is the next-finer tier's `max_mpp`; the finest tier is never culled
    // (a drop there would erase the feature at every zoom).
    let cull_mpp = (l.min_area_px > 0.0).then(|| config.lods.get(lod + 1).and_then(|n| n.max_mpp)).flatten();
    LodSet {
        lod,
        feats,
        simplified,
        presimplified,
        buckets,
        tol,
        line_tol,
        cull_mpp,
        min_area_px: l.min_area_px,
        cell_log2,
    }
}

/// Degree bounds → µdeg, widened outward so a candidate is never missed to a rounding step.
fn bounds_to_udeg(b: Bounds) -> UBox {
    ((b.0 * 1e6).floor() as i64, (b.1 * 1e6).floor() as i64, (b.2 * 1e6).ceil() as i64, (b.3 * 1e6).ceil() as i64)
}

impl LodSet<'_> {
    /// This level's quadtree for one cell: simplify, clip at the exact cell edge, cull the clipped
    /// geometry, then build the tree over the cell square.
    pub(super) fn cell_tree(&self, cell: CellId, chunk_size: usize, progress: &Progress) -> Node {
        debug_assert_eq!(cell.log2, self.cell_log2);
        let square = cell.square();
        let dbox = (square.0 as f64 / 1e6, square.1 as f64 / 1e6, square.2 as f64 / 1e6, square.3 as f64 / 1e6);
        let candidates = self.buckets.get(&(cell.i, cell.j)).map(Vec::as_slice).unwrap_or(&[]);
        let mut out: Vec<(u8, Geom)> = Vec::new();
        for &k in candidates {
            let (style_id, geom) = &self.feats[k as usize];
            let tolerance = if geom.is_lineal() { self.line_tol } else { self.tol };
            let simplified = if tolerance > 0.0 && !self.presimplified[k as usize] {
                match self.simplified[k as usize].as_deref() {
                    Some(cache) => Cow::Borrowed(cache.get_or_init(|| topology_preserve_simplify(geom, tolerance))),
                    None => Cow::Owned(topology_preserve_simplify(geom, tolerance)),
                }
            } else {
                Cow::Borrowed(geom.as_ref())
            };
            if simplified.is_empty() {
                continue;
            }
            let b = simplified.bounds();
            let clipped = if b.0 >= dbox.0 && b.2 <= dbox.2 && b.1 >= dbox.1 && b.3 <= dbox.3 {
                simplified.into_owned() // wholly inside: no clip, no vertex touched
            } else if b.2 < dbox.0 || b.0 > dbox.2 || b.3 < dbox.1 || b.1 > dbox.3 {
                continue; // a bounds-only candidate that the simplify moved out of reach
            } else {
                clip_to_box(&simplified, square)
            };
            // A semantic coverage feature has its minimum face size already; culling the clipped
            // result would re-open gaps the coverage closed. The hole trim still runs, at the same
            // threshold as everywhere else.
            let from_coverage = self.presimplified[k as usize];
            flatten_culled(*style_id, clipped, self.cull_mpp, self.min_area_px, from_coverage, &mut out);
        }
        build_lod_with(out, square, chunk_size, progress)
    }
}

/// Append `geom`'s simple parts to `out`, dropping the ones the sub-pixel footprint cull rejects and
/// trimming sub-pixel holes from the survivors — the pipeline's cull, applied to clipped geometry.
///
/// `from_coverage` skips the footprint cull entirely. A semantic coverage polygon clipped by a cell
/// edge can come out as a hairline strip along the seam, far under `min_area_px`, and dropping it
/// would open a backdrop sliver at the cell boundary, where the neighbouring cell still paints its
/// half.
fn flatten_culled(
    style_id: u8,
    geom: Geom,
    cull_mpp: Option<f64>,
    min_area_px: f64,
    from_coverage: bool,
    out: &mut Vec<(u8, Geom)>,
) {
    match geom {
        Geom::Empty => {}
        Geom::Multi(parts) => {
            for p in parts {
                flatten_culled(style_id, p, cull_mpp, min_area_px, from_coverage, out);
            }
        }
        mut simple => {
            if let Some(mpp) = cull_mpp {
                if !from_coverage && footprint_below(&simple, mpp, min_area_px) {
                    return;
                }
                strip_small_holes(&mut simple, mpp, min_area_px);
            }
            out.push((style_id, simple));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut_ingested, CutOptions};
    use crate::grid::BandTable;
    use crate::nav::NavGraph;
    use rayon::prelude::*;

    const LOG2: u32 = 18;

    #[test]
    fn land_backdrop_is_kept_for_preparation_then_removed_from_cell_geometry() {
        let config = Config::parse(
            r#"{
                "features":{"natural":{
                    "sea":{"z_index":1,"color":"0x001f"},
                    "land":{"z_index":0,"color":"0xffff"}
                }}
            }"#,
        )
        .unwrap();
        let polygon = |style_id| IngestFeature {
            style_id,
            min_lod: 0,
            geom: Geom::Polygon {
                exterior: vec![(0.0, 0.0), (0.1, 0.0), (0.1, 0.1), (0.0, 0.1), (0.0, 0.0)],
                interiors: Vec::new(),
            },
        };
        let ing = Ingested {
            landmark_links: Vec::new(),
            features: vec![polygon(2), polygon(1)], // land first, explicit sea second
            coastlines: Vec::new(),
            pois: Vec::new(),
            nav_graph: NavGraph::default(),
        };
        let semantic_scheme = config.semantic_scheme();
        let set = prepare_lod(
            &ing,
            &config,
            0,
            LOG2,
            PreparedSemantic { features: None, scheme: &semantic_scheme },
            &mut HashMap::new(),
            &Progress::silent(),
        );
        assert_eq!(set.feats.iter().map(|(style_id, _)| *style_id).collect::<Vec<_>>(), [1]);
        assert_eq!(set.presimplified.len(), set.feats.len(), "parallel preparation metadata stays aligned");
    }

    #[test]
    fn lods_share_merges_and_cells_share_simplification() {
        let config = Config::parse(
            r#"{
            "lods":[{"simplify":10}, {"max_mpp":4,"simplify":1}, {"max_mpp":2,"simplify":0.5}],
            "merge_lines":true,
            "features":{"highway":{"residential":{"color":"0xffff"}}}
        }"#,
        )
        .unwrap();
        let line = |min_lod, lat| IngestFeature {
            style_id: 1,
            min_lod,
            geom: Geom::Line(vec![(7.59, lat), (7.602, lat + 0.00004), (7.62, lat)]),
        };
        let ing = Ingested {
            features: vec![
                line(0, 47.3),
                line(0, 47.31),
                IngestFeature { style_id: 1, min_lod: 2, geom: Geom::Line(vec![(7.63, 47.4), (7.64, 47.4)]) },
                IngestFeature { style_id: 1, min_lod: 2, geom: Geom::Line(vec![(7.59, 47.33), (7.602176, 47.33)]) },
            ],
            landmark_links: Vec::new(),
            coastlines: Vec::new(),
            pois: Vec::new(),
            nav_graph: NavGraph::default(),
        };
        let scheme = config.semantic_scheme();
        let mut memo = HashMap::new();
        let mut sets: Vec<_> = (0..3)
            .map(|lod| {
                prepare_lod(
                    &ing,
                    &config,
                    lod,
                    LOG2,
                    PreparedSemantic { features: None, scheme: &scheme },
                    &mut memo,
                    &Progress::silent(),
                )
            })
            .collect();
        assert!(Arc::ptr_eq(&sets[0].feats, &sets[1].feats));
        assert!(!Arc::ptr_eq(&sets[1].feats, &sets[2].feats));
        assert_eq!(memo.len(), 2);
        let cells = [CellId::containing(LOG2, 47_300_000, 7_600_000), CellId::containing(LOG2, 47_300_000, 7_610_000)];
        assert_ne!(cells[0], cells[1]);
        assert_eq!(sets[2].simplified.iter().filter(|cache| cache.is_none()).count(), 1);
        for set in &sets {
            assert!(set.simplified[0].as_deref().unwrap().get().is_none());
            cells.par_iter().for_each(|cell| {
                set.cell_tree(*cell, 4096, &Progress::silent());
            });
        }
        let vertices = |set: &LodSet<'_>| match set.simplified[0].as_deref().unwrap().get().unwrap() {
            Geom::Line(points) => points.len(),
            geom => panic!("expected a simplified line: {geom:?}"),
        };
        assert_eq!(vertices(&sets[0]), 2);
        assert_eq!(vertices(&sets[1]), 3);

        for set in &mut sets {
            let cached: Vec<_> = cells
                .iter()
                .map(|cell| crate::serialize::serialize_tree(&set.cell_tree(*cell, 4096, &Progress::silent()), 4096))
                .collect();
            set.simplified.iter_mut().for_each(|cache| *cache = None);
            for (cell, expected) in cells.iter().zip(cached) {
                let tree = set.cell_tree(*cell, 4096, &Progress::silent());
                assert_eq!(crate::serialize::serialize_tree(&tree, 4096), expected);
            }
        }

        let bands = BandTable::parse(
            r#"{"bands":[
            {"id":"coarse","cell_log2":20,"lods":[0],"role":"coarse"},
            {"id":"fine","cell_log2":18,"lods":[1,2],"role":"geometry"},
            {"id":"network","cell_log2":18,"lods":[],"sections":["nav","poi"],"role":"core"}
        ]}"#,
        )
        .unwrap();
        let merges = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&merges);
        let progress = Progress::new(crate::progress::CancelToken::new(), move |_, line| {
            if line.contains("line fragment(s)") {
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let out = obcm_testkit::scratch::scratch_dir("obc-cut", "merge-memo");
        cut_ingested(&ing, &[], &config, &out, &CutOptions { bands, ..Default::default() }, &progress).unwrap();
        std::fs::remove_dir_all(out).unwrap();
        assert_eq!(
            merges.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "the two input sets merge once each across both geometry bands"
        );
    }
}
