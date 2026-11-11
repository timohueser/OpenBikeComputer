//! The face plans over every cell of the TrailView fixture: the planar partition (check 1), the same
//! borders in two neighbouring cells (check 2) and the same bytes twice (check 4); and the land
//! between the legs of a hairpin of the fixture.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::check::{artefacts, feature_names, neighbours, partition};
use obc_ground_plan::faces::{self, window, FacePlan, Rules, Surface};
use obc_ground_plan::frame::{from_lon_lat, CellKey};
use obc_ground_plan::graph::{self, WayGraph};

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
    let built = fixture.map(&cells, |key, input| {
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        let plan = faces::build(&graph, &input, rules, key, 0, true);
        let faults: Vec<String> =
            partition(&plan, window(key, 0.0)).into_iter().map(|f| format!("{key:?} 1: {}", f.what)).collect();
        let thin = artefacts(&plan, &graph).len();
        (key, plan, graph, feature_names(&input), faults, thin)
    });
    let mut plans: BTreeMap<CellKey, (FacePlan, WayGraph, Vec<[u32; 3]>)> = BTreeMap::new();
    let mut faults = Vec::new();
    let mut thin = 0;
    for (key, plan, graph, names, found, n) in built {
        faults.extend(found);
        thin += n;
        plans.insert(key, (plan, graph, names));
    }
    println!("{} cells in {:.1} s, {thin} artefacts", cells.len(), start.elapsed().as_secs_f64());
    let mut borders = 0;
    for (&key, a) in &plans {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some(b) = plans.get(&other) {
                borders += 1;
                let found = neighbours((&a.0, &a.1, &a.2), (&b.0, &b.1, &b.2), east);
                faults.extend(found.into_iter().map(|f| format!("{key:?}|{other:?} 2: {}", f.what)));
            }
        }
    }
    println!("{borders} borders");
    let again: Vec<CellKey> = cells.iter().step_by(17).copied().collect();
    let differ = fixture.map(&again, |key, input| {
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        (format!("{:?}", faces::build(&graph, &input, rules, key, 0, true)) != format!("{:?}", plans[&key].0))
            .then(|| format!("{key:?} 4: a second build differs"))
    });
    faults.extend(differ.into_iter().flatten());
    for check in [" 1: ", " 2: ", " 4: "] {
        let of: Vec<&String> = faults.iter().filter(|f| f.contains(check)).collect();
        println!("check{check}{} faults", of.len());
        for f in of.iter().take(8) {
            println!("  {f}");
        }
    }
    assert!(faults.is_empty(), "{} faults", faults.len());
}

/// The surface of a plan at a global point.
fn surface_at(plan: &FacePlan, lat: f64, lon: f64) -> Surface {
    let g = from_lon_lat(lon, lat);
    let o = plan.cell.origin();
    let q = [2 * (g.x - o.x), 2 * (g.y - o.y)];
    let inside = |ring: &[u32]| {
        let mut w = 0;
        for i in 0..ring.len() {
            let a = plan.vertices[ring[i] as usize].map(|v| 2 * i64::from(v));
            let b = plan.vertices[ring[(i + 1) % ring.len()] as usize].map(|v| 2 * i64::from(v));
            let side =
                i128::from(b[0] - a[0]) * i128::from(q[1] - a[1]) - i128::from(q[0] - a[0]) * i128::from(b[1] - a[1]);
            if a[1] <= q[1] && b[1] > q[1] && side > 0 {
                w += 1;
            } else if a[1] > q[1] && b[1] <= q[1] && side < 0 {
                w -= 1;
            }
        }
        w
    };
    let face = plan.faces.iter().find(|f| f.rings.iter().map(|r| inside(r)).sum::<i32>() != 0);
    face.map_or(Surface::Terrain, |f| f.surface)
}

#[test]
fn the_land_between_the_legs_of_the_i07_hairpin_stays_land_past_its_turn() {
    let mut fixture = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    // Between the legs of its two hairpins, past their turns, and the lower turn itself.
    let between = [(46.8217509, 8.4048884), (46.8217678, 8.4050035)];
    let turn = (46.8216499, 8.4049561);
    let key = CellKey::of(from_lon_lat(turn.1, turn.0));
    let input = fixture.cell(key);
    let graph = graph::build(key, &input, &rules.sections, &rules.structures);
    let plan = faces::build(&graph, &input, rules, key, 0, true);
    for (lat, lon) in between {
        let s = surface_at(&plan, lat, lon);
        assert!(!matches!(s, Surface::Carriageway | Surface::Crossing), "{s:?} at {lat}, {lon}");
    }
    assert_eq!(surface_at(&plan, turn.0, turn.1), Surface::Carriageway);
}
