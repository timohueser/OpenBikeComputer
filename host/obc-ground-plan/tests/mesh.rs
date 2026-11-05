//! The near mesh over every cell of the TrailView fixture: valid topology, the records' labels, and
//! two neighbouring cells with the same corners and heights on their shared line.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::{records, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::graph;
use obc_ground_plan::mesh::{check, mesh_cell, Rim};

#[test]
fn every_fixture_cell_meshes_validly_with_the_records_labels_and_agrees_with_its_neighbours() {
    let dir = default_dir();
    let mut fixture = Fixture::open(&dir);
    let rules = Rules::embedded();
    let cells: Vec<CellKey> = (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect();
    let built = fixture.map(&cells, |key, input| {
        let graph = graph::build(key, &input, &rules.sections);
        let built = records::build(&graph, &input, rules, key, 0, true);
        (key, Rim::of(&built.faces), built.faces, built.near.words, input.terrain)
    });
    let rims: BTreeMap<CellKey, Rim> = built.iter().map(|b| (b.0, b.1.clone())).collect();
    let mut faults = Vec::new();
    let mut meshes = BTreeMap::new();
    let mut labels = check::Labels::default();
    for (key, _, plan, words, terrain) in &built {
        let around: Vec<Rim> = (-1..=1i64)
            .flat_map(|dy| {
                (-1..=1i64).map(move |dx| CellKey { x: (key.x as i64 + dx) as u32, y: (key.y as i64 + dy) as u32 })
            })
            .filter_map(|k| rims.get(&k).cloned())
            .collect();
        let mesh = match mesh_cell(plan, words, &around, &mut || 0.0) {
            Ok(m) => m,
            Err(e) => {
                faults.push(format!("{key:?}: {e:?}"));
                continue;
            }
        };
        let s = &mesh.stats;
        if s.outside_d + s.unlabelled + s.split_edges > 0 {
            faults.push(format!("{key:?}: {s:?}"));
        }
        faults.extend(check::validity(&mesh).into_iter().map(|f| format!("{key:?}: {f}")));
        let l = check::labels(&mesh, words);
        labels.sharp_wrong += l.sharp_wrong;
        labels.ground_wrong += l.ground_wrong;
        labels.ground_other += l.ground_other;
        labels.first.extend(l.first.into_iter().map(|f| format!("{key:?} {f}")));
        meshes.insert(*key, (mesh, words, terrain));
    }
    let cell = |k: CellKey| {
        let (mesh, words, terrain) = &meshes[&k];
        check::Cell { key: k, mesh, words, terrain }
    };
    let (mut lines, mut sub_mm) = (0, 0);
    for &key in meshes.keys() {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if meshes.contains_key(&other) {
                let (f, s) = check::border(&cell(key), &cell(other), east);
                (lines, sub_mm) = (lines + 1, sub_mm + s);
                faults.extend(f);
            }
        }
    }
    println!(
        "{} cells meshed, {lines} shared lines ({sub_mm} points with a sub-mm step); labels {labels:?}",
        meshes.len()
    );
    assert!(faults.is_empty(), "{} faults: {:#?}", faults.len(), &faults[..faults.len().min(20)]);
    assert_eq!(labels.ground_wrong, 0, "{:#?}", labels.first);
}
