//! `merge.rs` — fill-dissolve (`merge_fills`): union polygons whose styles render
//! **pixel-identically** into one (multi)polygon, deleting every interior shared
//! boundary. A pure data-size / render-cost optimization with zero intended visual
//! change: an un-outlined fill next to an identical un-outlined fill already looks
//! like one blob today, but is stored as two polygons and drawn as two spans.
//!
//! Two styles render a fill identically iff they agree on `(z_index, color,
//! priority)` **and** neither carries a `color2` — a `color2` means the rings are
//! stroked (casing / outline, epic #556), so dissolving shared walls would change
//! the output. `weight`/`dashed` never affect an un-outlined polygon fill
//! (`obc-render`'s `fill_polygon_proj` consults only the color), so they are not in
//! the key. Merge candidates are selected by geometry **kind** (polygon), not by
//! style, because one style id can carry both lines and polygons.
//!
//! Placement (see `main.rs`): per LOD, **after** the `min_lod` filter and **before**
//! simplify — adjacent OSM parcels share boundary *nodes*, so the union dissolves
//! them exactly; simplifying first would move each copy of a shared boundary
//! independently and leave seam cracks. Off by default ⇒ byte-identical output.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::geom::{union_polygons, Geom};
use crate::serialize::Style;

/// A fill's render-equivalence key: `(z_index, color, priority)`. Two `color2`-less
/// styles sharing this key paint every fill pixel the same, so their polygons may be
/// unioned. `priority` is part of the key because it is the chunk-overflow drop class
/// — merging a prio-2 meadow into a prio-3 farmland would change which spans get
/// dropped under budget pressure.
pub type ClassKey = (i8, u16, u8);

/// `style_id → (class_key, canonical_style_id)` for every **mergeable** style
/// (`color2.is_none()`). The canonical id is the smallest style id in the class, so
/// it is deterministic and independent of which members appear at a given LOD; a
/// unioned group is tagged with it. Styles carrying a `color2` are absent from the
/// map and never merge.
pub fn merge_classes(styles: &[Style]) -> HashMap<u8, (ClassKey, u8)> {
    // Group mergeable styles by key so the canonical id can be the class minimum.
    let mut by_key: HashMap<ClassKey, Vec<u8>> = HashMap::new();
    for s in styles {
        if s.color2.is_none() {
            by_key.entry((s.z_index, s.color, s.priority)).or_default().push(s.id);
        }
    }
    let mut out = HashMap::with_capacity(styles.len());
    for (key, ids) in by_key {
        let canonical = ids.iter().copied().min().expect("a class has ≥1 member");
        for id in ids {
            out.insert(id, (key, canonical));
        }
    }
    out
}

/// Per-LOD merge counters, printed next to the footprint-cull line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MergeStats {
    /// Candidate polygons consumed by ≥2-member unions.
    pub merged_inputs: usize,
    /// Output parts those unions produced.
    pub merged_outputs: usize,
    /// Style classes that actually unioned (≥2 members, GEOS ok).
    pub merged_classes: usize,
    /// Singleton classes passed through byte-untouched (no GEOS round-trip).
    pub singletons: usize,
    /// Groups that hit a GEOS failure and passed through unmerged.
    pub fallbacks: usize,
}

/// Split a geometry into its polygon parts (merge candidates) and everything else
/// (lines pass through unchanged). Flattens nested `Multi`; drops `Empty`. In
/// practice only bare `Line`/`Polygon` reach the merge (Multi arises later, from
/// clipping), so the recursive/`others` arms are defensive.
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

/// One emission slot in input order: either a passthrough feature emitted here, or a
/// merge group's output emitted at its **first member's** position (later members of
/// the same group emit nothing).
enum Slot {
    Pass(u8, Geom),
    /// The class's canonical style id — its members live in `members[canonical]`.
    Group(u8),
}

/// Dissolve mergeable fill polygons in a per-LOD `(style_id, geom)` list.
///
/// Never drops a feature: a singleton class passes through byte-untouched (no GEOS
/// round-trip, so "flag on, no adjacent same-class polygons" is an empty diff), and
/// any GEOS failure on a ≥2-member group passes that group through unmerged with its
/// original style ids. Determinism: passthroughs keep their input order, each merged
/// group is emitted at its first member's position, group membership + union input
/// are walked in input order, and classes are keyed by canonical id — so packing the
/// same input twice is byte-identical.
pub fn merge_fills(features: Vec<(u8, Geom)>, classes: &HashMap<u8, (ClassKey, u8)>) -> (Vec<(u8, Geom)>, MergeStats) {
    // --- Phase 1: walk input, laying out slots and accumulating group members
    // (both in input order). ---
    let mut slots: Vec<Slot> = Vec::with_capacity(features.len());
    // canonical_id → members in input order, each keeping its original style id (a
    // singleton emits that id unchanged, so the byte-untouched guarantee holds even
    // when the class spans several style ids and only one appears at this LOD).
    let mut members: HashMap<u8, Vec<(u8, Geom)>> = HashMap::new();
    for (style_id, geom) in features {
        let Some(&(_key, canonical)) = classes.get(&style_id) else {
            slots.push(Slot::Pass(style_id, geom));
            continue;
        };
        // Mergeable style: candidates are the polygon parts; lines (and stray
        // non-polygon parts of a defensive Multi) pass through at this position.
        let mut polys = Vec::new();
        let mut others = Vec::new();
        split_geom(geom, &mut polys, &mut others);
        for o in others {
            slots.push(Slot::Pass(style_id, o));
        }
        if polys.is_empty() {
            continue;
        }
        let first = !members.contains_key(&canonical);
        let bucket = members.entry(canonical).or_default();
        for p in polys {
            bucket.push((style_id, p));
        }
        if first {
            slots.push(Slot::Group(canonical));
        }
    }

    // --- Phase 2: union each ≥2-member group in parallel. Each task builds, unions,
    // and reads back its GEOS geometries wholly on one thread — only plain `Geom`
    // crosses threads. Output order is set by phase 3, so the map's iteration order
    // is irrelevant. ---
    let mut unions: HashMap<u8, Option<Vec<Geom>>> = members
        .par_iter()
        .filter(|(_, m)| m.len() >= 2)
        .map(|(&canonical, m)| {
            let refs: Vec<&Geom> = m.iter().map(|(_, g)| g).collect();
            (canonical, union_polygons(&refs))
        })
        .collect();

    // --- Phase 3: emit in slot order. ---
    let mut out = Vec::with_capacity(slots.len());
    let mut stats = MergeStats::default();
    for slot in slots {
        match slot {
            Slot::Pass(sid, g) => out.push((sid, g)),
            Slot::Group(canonical) => {
                let group = members.remove(&canonical).expect("a Group slot has members");
                if group.len() == 1 {
                    // Singleton: byte-untouched, original style id and geometry.
                    stats.singletons += 1;
                    out.push(group.into_iter().next().unwrap());
                    continue;
                }
                let n_in = group.len();
                match unions.remove(&canonical).flatten() {
                    Some(parts) => {
                        stats.merged_classes += 1;
                        stats.merged_inputs += n_in;
                        stats.merged_outputs += parts.len();
                        for p in parts {
                            out.push((canonical, p));
                        }
                    }
                    // GEOS failed (or emptied a non-empty group): pass through
                    // unmerged, original style ids. Never drop map content.
                    None => {
                        stats.fallbacks += 1;
                        out.extend(group);
                    }
                }
            }
        }
    }
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fill-only style (no `color2`) at the given key fields; `weight`/`dashed`
    /// are set to non-default values to prove they never enter the class key.
    fn fill_style(id: u8, z_index: i8, color: u16, priority: u8) -> Style {
        Style { id, z_index, color, weight: 7, priority, dashed: true, color2: None }
    }

    /// A closed square ring `[o, o+s]²` (µdeg-friendly degrees), first == last.
    fn square(ox: f64, oy: f64, s: f64) -> Geom {
        Geom::Polygon {
            exterior: vec![(ox, oy), (ox + s, oy), (ox + s, oy + s), (ox, oy + s), (ox, oy)],
            interiors: vec![],
        }
    }

    fn total_area(features: &[(u8, Geom)]) -> f64 {
        // Shoelace over every polygon exterior minus its holes; lines contribute 0.
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
        for (_, g) in features {
            if let Geom::Polygon { exterior, interiors } = g {
                sum += ring_area(exterior);
                for h in interiors {
                    sum -= ring_area(h);
                }
            }
        }
        sum
    }

    fn count_polys(features: &[(u8, Geom)]) -> usize {
        features.iter().filter(|(_, g)| matches!(g, Geom::Polygon { .. })).count()
    }

    // --- merge_classes ------------------------------------------------------

    #[test]
    fn classes_group_by_key_and_pick_the_min_id() {
        // ids 3 and 7 share (z,color,prio); id 5 differs in color; id 9 has a color2.
        let styles = [
            fill_style(3, 2, 0x1234, 3),
            fill_style(7, 2, 0x1234, 3),
            fill_style(5, 2, 0x9999, 3),
            Style { color2: Some(0x0001), ..fill_style(9, 2, 0x1234, 3) },
        ];
        let classes = merge_classes(&styles);
        assert_eq!(classes[&3], ((2, 0x1234, 3), 3), "canonical is the smallest id in the class");
        assert_eq!(classes[&7], ((2, 0x1234, 3), 3));
        assert_eq!(classes[&5].1, 5, "a lone key is its own canonical");
        assert!(!classes.contains_key(&9), "a color2 style is never mergeable");
    }

    // --- merge_fills: the happy paths ---------------------------------------

    #[test]
    fn two_squares_sharing_an_edge_merge_and_lose_the_seam() {
        // Adjacent unit squares [0,1]×[0,1] and [1,2]×[0,1], same style.
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let a = square(0.0, 0.0, 1.0);
        let b = square(1.0, 0.0, 1.0);
        let in_verts: usize = [&a, &b].iter().map(|g| verts(g)).sum();
        let (out, stats) = merge_fills(vec![(1, a), (1, b)], &classes);
        assert_eq!(count_polys(&out), 1, "the two squares dissolve into one polygon");
        assert_eq!(out[0].0, 1, "tagged with the canonical (only) style id");
        assert!(verts(&out[0].1) < in_verts, "the shared edge's interior vertices are gone");
        assert_eq!((stats.merged_inputs, stats.merged_outputs, stats.merged_classes), (2, 1, 1));
    }

    #[test]
    fn two_styles_in_one_class_merge_to_the_canonical_id() {
        // ids 4 and 2 share a key ⇒ canonical 2; adjacent squares merge under id 2.
        let classes = merge_classes(&[fill_style(4, 1, 0xABCD, 2), fill_style(2, 1, 0xABCD, 2)]);
        let (out, _) = merge_fills(vec![(4, square(0.0, 0.0, 1.0)), (2, square(1.0, 0.0, 1.0))], &classes);
        assert_eq!(count_polys(&out), 1);
        assert_eq!(out[0].0, 2, "merged part carries the class's canonical (smallest) style id");
    }

    #[test]
    fn ring_of_parcels_merges_to_a_polygon_with_one_hole() {
        // Eight unit squares tiling the border of a 3×3 grid, centre cell unmapped.
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let mut feats = Vec::new();
        for gx in 0..3 {
            for gy in 0..3 {
                if gx == 1 && gy == 1 {
                    continue; // leave the middle empty
                }
                feats.push((1u8, square(gx as f64, gy as f64, 1.0)));
            }
        }
        let (out, _) = merge_fills(feats, &classes);
        assert_eq!(count_polys(&out), 1, "the border tiles dissolve into one frame polygon");
        match &out[0].1 {
            Geom::Polygon { interiors, .. } => {
                assert_eq!(interiors.len(), 1, "the unmapped centre is exactly one hole")
            }
            other => panic!("expected a polygon, got {other:?}"),
        }
    }

    #[test]
    fn overlapping_duplicates_collapse_to_one() {
        // The same parcel mapped twice (identical geometry) → one polygon, area preserved.
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let dup = square(0.0, 0.0, 1.0);
        let (out, _) = merge_fills(vec![(1, dup.clone()), (1, dup)], &classes);
        assert_eq!(count_polys(&out), 1);
        assert!((total_area(&out) - 1.0).abs() < 1e-12, "double-mapped area is not doubled");
    }

    #[test]
    fn disjoint_same_class_polygons_merge_without_losing_area() {
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let a = square(0.0, 0.0, 1.0);
        let b = square(5.0, 5.0, 1.0); // far apart, no shared boundary
        let (out, stats) = merge_fills(vec![(1, a), (1, b)], &classes);
        assert!((total_area(&out) - 2.0).abs() < 1e-12, "disjoint areas both survive");
        assert_eq!(stats.merged_inputs, 2, "both counted as merged inputs");
    }

    // --- merge_fills: what must NOT merge -----------------------------------

    #[test]
    fn each_key_dimension_blocks_merging() {
        // Base style id 1; three others differ in exactly one key field, plus one with a color2.
        let base = fill_style(1, 0, 0x00F0, 3);
        let styles = [
            base,
            fill_style(2, 1, 0x00F0, 3), // different z_index
            fill_style(3, 0, 0x00F1, 3), // different color
            fill_style(4, 0, 0x00F0, 2), // different priority
            Style { color2: Some(0x0001), ..fill_style(5, 0, 0x00F0, 3) }, // has a color2
        ];
        let classes = merge_classes(&styles);
        for other in [2u8, 3, 4, 5] {
            let (out, stats) = merge_fills(vec![(1, square(0.0, 0.0, 1.0)), (other, square(1.0, 0.0, 1.0))], &classes);
            assert_eq!(count_polys(&out), 2, "style {other} must not merge with style 1");
            assert_eq!(stats.merged_classes, 0, "no union happened for style {other}");
        }
    }

    #[test]
    fn a_line_in_a_merge_class_passes_through_as_a_line() {
        // Style 1 is in a class, but this feature is a line (kind, not style, decides).
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let line = Geom::Line(vec![(0.0, 0.0), (1.0, 1.0)]);
        let (out, stats) = merge_fills(vec![(1, line.clone()), (1, square(0.0, 0.0, 1.0))], &classes);
        assert!(out.iter().any(|(_, g)| matches!(g, Geom::Line(_))), "the line survives as a line");
        assert_eq!(stats.singletons, 1, "the lone square is a singleton (the line is not a candidate)");
    }

    #[test]
    fn singleton_group_is_byte_untouched() {
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let poly = square(0.25, 0.25, 0.5);
        let (out, stats) = merge_fills(vec![(1, poly.clone())], &classes);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 1);
        assert_eq!(verts(&out[0].1), verts(&poly), "no GEOS round-trip: vertex count unchanged");
        assert!((total_area(&out) - total_area(&[(1, poly)])).abs() < 1e-15, "geometry identical");
        assert_eq!(stats, MergeStats { singletons: 1, ..Default::default() });
    }

    #[test]
    fn a_style_outside_any_class_passes_through() {
        // Empty class table ⇒ nothing is mergeable ⇒ input echoes back unchanged.
        let (out, stats) = merge_fills(vec![(9, square(0.0, 0.0, 1.0)), (9, square(2.0, 0.0, 1.0))], &HashMap::new());
        assert_eq!(count_polys(&out), 2, "no class ⇒ no merge");
        assert_eq!(stats, MergeStats::default());
    }

    // --- ordering & determinism ---------------------------------------------

    #[test]
    fn merged_group_sits_at_its_first_members_position() {
        // Input [A(class1), B(other), C(class1)] → [merged(class1), B]: the merged
        // block sits at A's position, B keeps its place after it.
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3)]);
        let feats = vec![
            (1u8, square(0.0, 0.0, 1.0)), // A
            (9u8, square(0.0, 5.0, 1.0)), // B — style 9 not in any class
            (1u8, square(1.0, 0.0, 1.0)), // C, shares an edge with A
        ];
        let (out, _) = merge_fills(feats, &classes);
        assert_eq!(out.len(), 2, "A+C merged into one, B passthrough");
        assert_eq!(out[0].0, 1, "the merged block is first (A's position)");
        assert!(matches!(out[0].1, Geom::Polygon { .. }));
        assert_eq!(out[1].0, 9, "B keeps its position after the merged block");
    }

    #[test]
    fn packing_the_same_input_twice_is_identical() {
        let classes = merge_classes(&[fill_style(1, 0, 0x00F0, 3), fill_style(2, 1, 0x1111, 3)]);
        let build = || {
            vec![
                (1u8, square(0.0, 0.0, 1.0)),
                (2u8, square(0.0, 3.0, 1.0)),
                (1u8, square(1.0, 0.0, 1.0)),
                (2u8, square(1.0, 3.0, 1.0)),
            ]
        };
        let (a, sa) = merge_fills(build(), &classes);
        let (b, sb) = merge_fills(build(), &classes);
        assert_eq!(sa, sb);
        let key = |v: &[(u8, Geom)]| v.iter().map(|(s, g)| (*s, verts(g))).collect::<Vec<_>>();
        assert_eq!(key(&a), key(&b), "same style-id + vertex-count sequence both runs");
    }

    /// Total vertex count across a geometry's rings (exterior + holes / parts).
    fn verts(g: &Geom) -> usize {
        match g {
            Geom::Line(c) => c.len(),
            Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
            Geom::Multi(parts) => parts.iter().map(verts).sum(),
            Geom::Empty => 0,
        }
    }
}
