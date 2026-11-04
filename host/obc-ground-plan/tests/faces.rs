//! The face plans over every cell of the TrailView fixture: the planar partition (check 1), the same
//! borders in two neighbouring cells (check 2), every corner within its extent and ramp cap (check 3),
//! and the same bytes twice (check 4).

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::check::{neighbours, partition};
use obc_ground_plan::faces::{build_faces, window, FacePlan, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::graph::{build, WayGraph};

#[test]
fn every_fixture_cell_is_a_planar_partition_that_its_neighbours_agree_with() {
    let mut fixture = Fixture::open(&default_dir());
    let dir = default_dir();
    let rules = Rules::embedded();
    let cells: Vec<CellKey> = (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect();
    assert!(cells.len() > 100, "{} cells", cells.len());
    let start = std::time::Instant::now();
    let mut plans: BTreeMap<CellKey, (FacePlan, WayGraph)> = BTreeMap::new();
    let mut faults = Vec::new();
    for &key in &cells {
        let input = fixture.cell(key);
        let graph = build(key, &input, &rules.sections);
        let plan = build_faces(&graph, &input, rules, key, 0, true);
        faults.extend(
            partition(&plan, window(key, rules.plan.window_m)).into_iter().map(|f| format!("{key:?} 1: {}", f.what)),
        );
        if plan.stats.extent_faults > 0 {
            faults.push(format!("{key:?} 3: {} points past their extent", plan.stats.extent_faults));
        }
        plans.insert(key, (plan, graph));
    }
    println!("{} cells in {:.1} s", cells.len(), start.elapsed().as_secs_f64());
    let mut borders = 0;
    for (&key, a) in &plans {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some(b) = plans.get(&other) {
                borders += 1;
                let found = neighbours((&a.0, &a.1), (&b.0, &b.1), east);
                faults.extend(found.into_iter().map(|f| format!("{key:?}|{other:?} 2: {}", f.what)));
            }
        }
    }
    println!("{borders} borders");
    for key in cells.iter().step_by(17) {
        let input = fixture.cell(*key);
        let graph = build(*key, &input, &rules.sections);
        let again = build_faces(&graph, &input, rules, *key, 0, true);
        if format!("{again:?}") != format!("{:?}", plans[key].0) {
            faults.push(format!("{key:?} 4: a second build differs"));
        }
    }
    for check in [" 1: ", " 2: ", " 3: ", " 4: "] {
        let of: Vec<&String> = faults.iter().filter(|f| f.contains(check)).collect();
        let pinches = of.iter().filter(|f| f.contains("pinches left")).count();
        println!("check{check}{} faults ({pinches} of them pinches left)", of.len());
        for f in of.iter().filter(|f| !f.contains("pinches left")).take(8) {
            println!("  {f}");
        }
    }
    assert!(faults.is_empty(), "{} faults", faults.len());
}
