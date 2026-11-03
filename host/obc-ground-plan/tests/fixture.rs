//! The crate's base over the TrailView fixture: its tiles decode and agree where they overlap, each
//! element of a cell has one owner, and the terrain is known everywhere.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use fixture::{default_dir, Fixture};
use obc_ground_plan::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use obc_ground_plan::tile::{element_count, features, segment_meets, tile_square, Feature, Geometry, Part};

const COLUMNS: std::ops::RangeInclusive<u32> = 4286..=4288;
const ROWS: std::ops::RangeInclusive<u32> = 2887..=2889;

/// (kind, tags, the two end points): a segment's key, the same in every tile that holds it.
type Key = (String, String, [i64; 4]);

fn ends(geometry: Geometry, points: &[GlobalPoint], i: usize) -> (GlobalPoint, GlobalPoint) {
    match geometry {
        Geometry::Point => (points[i], points[i]),
        Geometry::Line => (points[i], points[i + 1]),
        Geometry::Area => (points[i], points[(i + 1) % points.len()]),
    }
}

fn key(feature: &Feature, (a, b): (GlobalPoint, GlobalPoint)) -> Key {
    (feature.kind.clone(), format!("{:?}", feature.tags), [a.x, a.y, b.x, b.y])
}

/// Each element of each part with its index: (feature, part, element).
fn elements(features: &[Feature]) -> impl Iterator<Item = (&Feature, &Part, usize)> {
    features.iter().flat_map(|f| {
        f.parts.iter().flat_map(move |p| (0..element_count(f.geometry, p.points.len())).map(move |i| (f, p, i)))
    })
}

fn tiles() -> BTreeMap<(u32, u32), Vec<Feature>> {
    let dir = default_dir();
    let mut out = BTreeMap::new();
    for column in COLUMNS {
        for row in ROWS {
            let bytes = fixture::feature_tile(&dir, column, row).expect("a fixture tile");
            out.insert((column, row), features(&bytes, column, row).unwrap_or_else(|e| panic!("{column}/{row}: {e}")));
        }
    }
    out
}

/// What two tiles must agree on at a vertex: the height and the alignment but `along_m`, with the
/// tangent as its code.
fn vertex(part: &Part, v: usize) -> String {
    let height = part.heights.as_ref().map(|h| h[v]);
    let plan = part.plan.as_ref().map(|p| {
        let turn = (p.tangent[v] * 1024.0 / std::f64::consts::TAU).round() as i64 % 1024;
        (turn, p.grade[v], p.bank[v], p.widen[v], p.side[v])
    });
    format!("{height:?} {plan:?}")
}

/// The values of an element: those of its end vertices, and the walk of a segment.
fn values(feature: &Feature, part: &Part, i: usize) -> String {
    let next = if feature.geometry == Geometry::Area {
        (i + 1) % part.points.len()
    } else {
        (i + 1).min(part.points.len() - 1)
    };
    let walk = (feature.geometry == Geometry::Line).then(|| part.plan.as_ref().map(|p| p.walk[i]));
    format!("{} | {} | {walk:?}", vertex(part, i), vertex(part, next))
}

/// Whether an element meets a closed square.
fn meets(feature: &Feature, part: &Part, i: usize, square: [i64; 4]) -> bool {
    let (a, b) = ends(feature.geometry, &part.points, i);
    segment_meets(a, b, square)
}

#[test]
fn every_fixture_tile_decodes() {
    for ((column, row), features) in tiles() {
        let mut kinds: BTreeMap<(&str, Geometry), (usize, usize, usize)> = BTreeMap::new();
        for f in &features {
            let count = kinds.entry((f.kind.as_str(), f.geometry)).or_default();
            count.0 += 1;
            count.1 += f.parts.len();
            count.2 += f.parts.iter().map(|p| p.points.len()).sum::<usize>();
        }
        println!("13/{column}/{row}: {} features", features.len());
        for ((kind, geometry), (features, parts, vertices)) in kinds {
            println!("  {kind} {geometry:?}: {features} features, {parts} parts, {vertices} vertices");
        }
        assert!(!features.is_empty());
    }
}

#[test]
fn neighbour_tiles_agree_where_both_own() {
    let tiles = tiles();
    let mut compared = 0;
    for (&a, features_a) in &tiles {
        for (&b, features_b) in &tiles {
            let (dx, dy) = (i64::from(b.0) - i64::from(a.0), i64::from(b.1) - i64::from(a.1));
            if a == b || dx.abs() > 1 || dy.abs() > 1 {
                continue;
            }
            let held: BTreeMap<Key, Vec<String>> = elements(features_b).fold(BTreeMap::new(), |mut map, (f, p, i)| {
                map.entry(key(f, ends(f.geometry, &p.points, i))).or_default().push(values(f, p, i));
                map
            });
            for (f, p, i) in elements(features_a) {
                // A ring's join of the cut lies outside its tile's buffered square: only its edges at
                // its own square are the ring's.
                if !meets(f, p, i, tile_square(b)) || f.geometry == Geometry::Area && !meets(f, p, i, tile_square(a)) {
                    continue;
                }
                let k = key(f, ends(f.geometry, &p.points, i));
                let copies = held.get(&k).unwrap_or_else(|| panic!("{a:?} holds {k:?}, {b:?} does not"));
                assert!(copies.contains(&values(f, p, i)), "{k:?}: {a:?} {} and {b:?} {copies:?}", values(f, p, i));
                compared += 1;
            }
        }
    }
    println!("{compared} elements compared");
    assert!(compared > 1000);
}

#[test]
fn each_element_has_one_owner() {
    let tiles = tiles();
    let mut fixture = Fixture::open(&default_dir());
    let inside = |key: CellKey| key.feature_tiles().iter().all(|t| tiles.contains_key(t));
    let cells: Vec<CellKey> = fixture.cells().into_iter().filter(|&k| inside(k)).collect();
    assert!(cells.len() > 100);
    for key in cells {
        let cell = fixture.cell(key);
        let mut owners: BTreeMap<Key, BTreeSet<(u32, u32)>> = BTreeMap::new();
        for (f, p, i) in elements(&cell.features) {
            if p.own[i] {
                owners.entry(key_of(f, p, i)).or_default().insert(p.tile);
            }
        }
        if let Some((k, from)) = owners.iter().find(|(_, from)| from.len() > 1) {
            panic!("{key:?}: tiles {from:?} each own {k:?}");
        }
        let grown = key.grown();
        for &t in &key.feature_tiles() {
            for (f, p, i) in elements(&tiles[&t]) {
                let real = f.geometry != Geometry::Area || meets(f, p, i, tile_square(t));
                if real && meets(f, p, i, grown) {
                    assert!(
                        owners.contains_key(&key_of(f, p, i)),
                        "{key:?}: no tile owns {:?} of {t:?}",
                        key_of(f, p, i)
                    );
                }
            }
        }
    }
}

fn key_of(f: &Feature, p: &Part, i: usize) -> Key {
    key(f, ends(f.geometry, &p.points, i))
}

#[test]
fn the_fixture_terrain_is_known_in_every_cell() {
    let mut fixture = Fixture::open(&default_dir());
    for key in fixture.cells() {
        let cell = fixture.cell(key);
        let step = (10.0 / metres_per_unit(key.origin().y)) as i64;
        let o = key.origin();
        for y in (o.y..o.y + CELL_UNITS).step_by(step as usize) {
            for x in (o.x..o.x + CELL_UNITS).step_by(step as usize) {
                let h = cell.terrain.height(GlobalPoint { x, y });
                assert!(h.is_some_and(|h| (500.0..3500.0).contains(&h)), "{key:?} at ({x}, {y}): {h:?}");
            }
        }
    }
}
