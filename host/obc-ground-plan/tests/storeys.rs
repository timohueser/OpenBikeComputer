//! The storeys over the ground over every cell of the TrailView fixture: storey 0 reads the same with them, each
//! storey is a planar partition, each shell part closes, no parapet or railing stands in a paved face of its
//! storey, and two neighbouring cells give a storey the same corners and heights on their shared line.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::cell::{build_request, build_storeys, CellBuild};
use obc_ground_plan::faces::check::partition;
use obc_ground_plan::faces::heights::height_at;
use obc_ground_plan::faces::records::find;
use obc_ground_plan::faces::{window, Rules};
use obc_ground_plan::frame::{CellKey, CELL_UNITS};
use obc_ground_plan::mesh::shell::{border, closure, mesh_storeys, on_paved, StoreyMesh};
use obc_ground_plan::tile::TileCache;

/// Points per side of the grid at which storey 0 must read the same with the storeys' records.
const GRID: i32 = 32;

#[test]
fn the_storeys_close_stand_on_their_sheets_meet_their_neighbours_and_leave_storey_0_as_it_was() {
    let mut fix = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let mut cache = TileCache::new(16);
    let mut faults = Vec::new();
    let mut built: BTreeMap<CellKey, (CellBuild, StoreyMesh)> = BTreeMap::new();
    for key in fix.cells() {
        let without = build_request(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        let with = build_storeys(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        assert_eq!(without.faces, with.faces, "{key:?}: storey 0's faces");
        assert_eq!(without.far, with.far, "{key:?}: the far block");
        if with.storeys.plans.is_empty() {
            assert_eq!(without.records, with.records, "{key:?}: a cell with no deck has the records of storey 0 alone");
            continue;
        }
        // Storey 0 reads the same label and height at a grid of points.
        let c = f64::from(CELL_UNITS as i32);
        for j in 0..GRID {
            for i in 0..GRID {
                let q = [(f64::from(i) + 0.37) * c / f64::from(GRID), (f64::from(j) + 0.61) * c / f64::from(GRID)];
                let (a, b) = (find(&without.records, q), find(&with.records, q));
                let (ha, hb) = (height_at(&without.records, q, a, 500.0).h, height_at(&with.records, q, b, 500.0).h);
                if a != b || ha.to_bits() != hb.to_bits() {
                    faults.push(format!(
                        "{key:?} at {q:?}: storey 0 reads {a:?} {ha} without and {b:?} {hb} with storeys"
                    ));
                }
            }
        }
        for plan in &with.storeys.plans {
            faults.extend(
                partition(plan, window(key, 0.0)).into_iter().map(|f| format!("{key:?} storey {}: {f:?}", plan.level)),
            );
        }
        let mesh = mesh_storeys(key, &with.storeys, &with.records, &rules.structures).unwrap();
        assert_eq!(mesh.stats.unlabelled, 0, "{key:?}: a sheet face has no label");
        faults.extend(closure(&mesh));
        faults.extend(on_paved(&mesh, &with.storeys, &with.records));
        built.insert(key, (with, mesh));
    }
    for (&key, (b, m)) in &built {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some((ob, om)) = built.get(&other) {
                faults.extend(border((m, &b.records), (om, &ob.records), east));
            }
        }
    }
    let triangles: u32 = built.values().map(|(_, m)| m.triangles.len() as u32).sum();
    println!("{} cells with storeys, {triangles} storey triangles, {} faults", built.len(), faults.len());
    assert!(built.len() > 10, "the fixture's decks");
    assert!(faults.is_empty(), "{} faults: {:#?}", faults.len(), &faults[..faults.len().min(20)]);
}
