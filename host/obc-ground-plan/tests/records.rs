//! The records over every cell of the TrailView fixture: each sampled point gets its face's label,
//! the far level keeps every label but the band layers', the far block holds the near lines in reach
//! and is the far-only build's, two neighbouring cells give one label on their shared line, and a
//! second build gives the same words.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::check::feature_names;
use obc_ground_plan::faces::records;
use obc_ground_plan::faces::records::check::{
    agreement, border, far_agreement, far_matches_near, outline_border, Overlay,
};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::graph::{self, WayGraph};

/// Sample points per bin side.
const SAMPLES: i32 = 5;
/// A miss this near an edge between two labels is the slice's precision: `MERGE` (4 units) and
/// the snap rounding of the overlay.
const EDGE_UNITS: f64 = 8.0;

/// A cell's near records, graph and feature names.
type Built = (Vec<u32>, WayGraph, Vec<[u32; 3]>);

#[test]
fn every_fixture_cell_gives_its_points_the_faces_labels_alike_near_far_and_across_borders() {
    let dir = default_dir();
    let mut fixture = Fixture::open(&dir);
    let rules = Rules::embedded();
    let cells: Vec<CellKey> = (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect();
    assert!(cells.len() > 100, "{} cells", cells.len());
    let built = fixture.map(&cells, |key, input| {
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        let built = records::build(&graph, &input, rules, key, 0, true);
        let (near, far, plan) = (built.near.words, built.far_records.words, built.faces);
        // The viewer's cell-only pass gives the records' faces.
        let alone = obc_ground_plan::faces::build(&graph, &input, rules, key, 0, true);
        let same = alone.vertices == plan.vertices && alone.faces == plan.faces && alone.edges == plan.edges;
        let a = agreement(&near, &plan, &Overlay::new(&graph, &input, rules, key, true), SAMPLES, EDGE_UNITS);
        let f = far_agreement(&near, &far, SAMPLES);
        let (mut lines, counts) = match &built.far_block {
            Ok(block) => far_matches_near(&near, block, key),
            Err(e) => (vec![e.clone()], BTreeMap::new()),
        };
        if records::far_block(&graph, &input, rules, key) != built.far_block {
            lines.push("the far-only build's far block differs".into());
        }
        let names = feature_names(&input);
        (key, near, graph, names, a, f, same, (lines, counts))
    });
    let (mut points, mut edge, mut arc, mut patch, mut faults) = (0, 0, 0, 0, Vec::new());
    let mut far = [0u64; 2];
    let mut blocks: BTreeMap<&str, usize> = BTreeMap::new();
    let mut cells_of: BTreeMap<CellKey, Built> = BTreeMap::new();
    for (key, near, graph, names, a, f, same, (lines, counts)) in built {
        if !same {
            faults.push(format!("{key:?}: faces::build differs from the records' faces"));
        }
        faults.extend(lines.iter().map(|l| format!("{key:?} far block: {l}")));
        counts.into_iter().for_each(|(case, n)| *blocks.entry(case).or_default() += n);
        (points, edge, arc, patch) = (points + a.points, edge + a.edge, arc + a.arc, patch + a.patch);
        faults
            .extend(a.area.iter().map(|m| format!("{key:?} at {:?}: face {:?}, records {:?}", m.q, m.face, m.records)));
        far = [far[0] + f[0], far[1] + f[1]];
        cells_of.insert(key, (near, graph, names));
    }
    println!("{points} points: {edge} near an edge, {arc} on an arc, {patch} by a patch rule; far {far:?}");
    println!("far blocks: {blocks:?}");
    let (mut borders, mut outline, mut past) = ((0, 0, 0), 0, Vec::new());
    for (&key, a) in &cells_of {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            let Some(b) = cells_of.get(&other) else { continue };
            let (n, blend, differ) = border((&a.0, &a.1, &a.2), (&b.0, &b.1, &b.2), east, 64);
            borders = (borders.0 + 1, borders.1 + n, borders.2 + blend);
            faults.extend(differ.iter().map(|d| format!("{key:?}|{other:?} at {}: {} | {}", d.0, d.1, d.2)));
            let (n, [near, far]) = outline_border((&a.0, key), (&b.0, other));
            outline += n;
            faults.extend(near.iter().map(|d| format!("{key:?}|{other:?} outline edge {d}")));
            past.extend(far);
        }
    }
    println!("{} borders, {} points, {} with a blend edge near", borders.0, borders.1, borders.2);
    // Past MARGIN_M the paved field at an outline edge's end is the neighbour's own only as far as this
    // shows (`records::outline_m`).
    println!(
        "{outline} outline edges near a shared line, {} differ past MARGIN_M: {:?}",
        past.len(),
        &past[..past.len().min(3)]
    );
    let again: Vec<CellKey> = cells.iter().step_by(17).copied().collect();
    let differ = fixture.map(&again, |key, input| {
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        (records::build(&graph, &input, rules, key, 0, true).near.words != cells_of[&key].0)
            .then(|| format!("{key:?}: a second build differs"))
    });
    faults.extend(differ.into_iter().flatten());
    assert_eq!(far[1], 0, "far labels differ on {} of {} points", far[1], far[0]);
    assert!(edge * 1000 < points, "{edge} of {points} points near an edge");
    assert!(faults.is_empty(), "{} faults: {:?}", faults.len(), &faults[..faults.len().min(8)]);
}
