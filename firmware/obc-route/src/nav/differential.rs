extern crate std;

#[allow(dead_code)]
#[path = "legacy.rs"]
mod legacy;

use super::{BikeType, NavError, NavPlanner, NavScratch, Objective, RouteStats, Step, NAV_MAX_NODES};
use obc_elevation::NullElevation;
use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use std::{boxed::Box, cell::RefCell, vec, vec::Vec};

struct Source<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<(u64, usize)>>,
}

impl ByteSource for Source<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.borrow_mut().push((offset, buf.len()));
        SliceSource(self.bytes).read_at(offset, buf)
    }

    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

#[derive(Default)]
struct Sink(Vec<u8>);

impl ByteSink for Sink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        self.0[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Running,
    Done(RouteStats),
    NoPath,
    Exhausted,
}

struct Comparison {
    outcome: Outcome,
    epsilon: (u32, u32),
    table_full_seen: bool,
}

fn compare<const N: usize>(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    bike: BikeType,
    objective: usize,
) -> Comparison {
    let sources = core::array::from_fn::<_, 2, _>(|_| Source { bytes, reads: RefCell::new(Vec::new()) });
    let tables = sources.each_ref().map(|src| MapTables::parse(src).unwrap());
    let caches = [MapCache::new(), MapCache::new()];
    let readers = core::array::from_fn::<_, 2, _>(|i| Reader::new(&sources[i], &tables[i], &caches[i]));
    let mut old = Box::new(legacy::NavPlanner::new(from, to, "Parity", bike));
    let mut new = Box::new(NavPlanner::new(from, to, "Parity", bike));
    old.set_objective(legacy::Objective::TRIALS[objective]);
    new.set_objective(Objective::TRIALS[objective]);
    let mut old_scratch = Box::new(legacy::NavScratch::<N>::new());
    let mut new_scratch = Box::new(NavScratch::<N>::new());
    let mut tiles = [NavTileCache::new(), NavTileCache::new()];
    let mut sinks = [Sink::default(), Sink::default()];
    for source in &sources {
        source.reads.borrow_mut().clear();
    }

    let mut table_full_seen = false;
    for step in 0..100_000 {
        let old_step = match old.step(&readers[0], &mut old_scratch, &mut tiles[0], &mut NullElevation, &mut sinks[0]) {
            legacy::Step::Running => Outcome::Running,
            legacy::Step::Done(stats) => Outcome::Done(stats),
            legacy::Step::Failed(legacy::NavError::NoPath) => Outcome::NoPath,
            legacy::Step::Failed(legacy::NavError::Exhausted) => Outcome::Exhausted,
        };
        let new_step = match new.step(&readers[1], &mut new_scratch, &mut tiles[1], &mut NullElevation, &mut sinks[1]) {
            Step::Running => Outcome::Running,
            Step::Done(stats) => Outcome::Done(stats),
            Step::Failed(NavError::NoPath) => Outcome::NoPath,
            Step::Failed(NavError::Exhausted) => Outcome::Exhausted,
        };
        assert_eq!(old_step, new_step, "N={N}, {from:?}->{to:?}, {bike:?}, objective={objective}, step={step}");
        assert_eq!(old.settles(), new.settles());
        assert_eq!(old.epsilon_used(), new.epsilon_used());
        assert_eq!(legacy::table_full(&old), new.table_full);
        table_full_seen |= new.table_full;
        assert_eq!(old.snapped_start(), new.snapped_start());
        assert_eq!(old.snapped_goal(), new.snapped_goal());
        assert_eq!(tiles[0].stats(), tiles[1].stats());
        assert_eq!(sinks[0].0, sinks[1].0);
        assert_eq!(*sources[0].reads.borrow(), *sources[1].reads.borrow());
        for source in &sources {
            source.reads.borrow_mut().clear();
        }
        if new_step != Outcome::Running {
            return Comparison { outcome: new_step, epsilon: new.epsilon_used(), table_full_seen };
        }
    }
    panic!("planner does not terminate");
}

fn next(seed: &mut u64) -> u32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (*seed >> 32) as u32
}

fn graph_map(disconnected: bool) -> (Vec<u8>, Vec<(i32, i32)>) {
    let mut graph = NavGraph { nodes: Vec::new(), edges: Vec::new() };
    for row in 0..9 {
        for col in 0..12 {
            graph
                .nodes
                .push(Node { id: (row * 12 + col) as u32, coord: (500_000 + col * 10_000, 500_000 + row * 10_000) });
        }
    }
    for row in 0..9 {
        for col in 0..12 {
            let a = (row * 12 + col) as usize;
            for b in [if col < 11 { Some(a + 1) } else { None }, if row < 8 { Some(a + 12) } else { None }]
                .into_iter()
                .flatten()
            {
                if disconnected && a % 12 == 5 && b == a + 1 {
                    continue;
                }
                let (ca, cb) = (graph.nodes[a].coord, graph.nodes[b].coord);
                let mid = ((ca.0 + cb.0) / 2 + 500, (ca.1 + cb.1) / 2 + 500);
                graph.edges.push(Edge {
                    a: a as u32,
                    b: b as u32,
                    polyline: vec![ca, mid, cb],
                    length_m: 1_200,
                    kind: 0,
                });
            }
        }
    }
    let mut points: Vec<_> = graph.nodes.iter().map(|node| node.coord).collect();
    points.extend(graph.edges.iter().map(|edge| edge.polyline[1]));
    let bbox = (0, 0, 1_000_000, 1_000_000);
    let lods = [LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox, features: Vec::new() } }];
    let profiles = [NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
    let (bytes, dropped) = serialize_lods(&lods, &[], 0xf800, bbox, &[], &graph, &profiles, &mut NullElevation);
    assert_eq!(dropped, 0);
    (bytes, points)
}

fn random_pairs(bytes: &[u8], points: &[(i32, i32)], count: usize) {
    let mut seed = 0x7257_3624_49;
    let mut outcomes = [0; 3];
    let mut record = |result: Comparison| {
        outcomes[match result.outcome {
            Outcome::Done(_) => 0,
            Outcome::NoPath => 1,
            Outcome::Exhausted => 2,
            Outcome::Running => unreachable!(),
        }] += 1;
    };
    for index in 0..count {
        let mut pick = || {
            let p = points[next(&mut seed) as usize % points.len()];
            let jitter = if index % 3 == 0 {
                (0, 0)
            } else {
                ((next(&mut seed) % 1_601) as i32 - 800, (next(&mut seed) % 1_601) as i32 - 800)
            };
            (p.0 + jitter.0, p.1 + jitter.1)
        };
        let (from, to) = (pick(), pick());
        for bike in BikeType::ALL {
            record(compare::<NAV_MAX_NODES>(bytes, from, to, bike, index % Objective::TRIALS.len()));
            if index < 16 {
                record(compare::<20>(bytes, from, to, bike, 0));
                record(compare::<50>(bytes, from, to, bike, 0));
            }
        }
    }
    assert!(outcomes[0] > 0, "the random pool must include successful plans");
    std::println!("planner differential: {} done, {} NoPath, {} Exhausted", outcomes[0], outcomes[1], outcomes[2]);
}

#[test]
fn synthetic_plans_match_at_every_step() {
    for disconnected in [false, true] {
        let (bytes, points) = graph_map(disconnected);
        random_pairs(&bytes, &points, 64);
        assert_eq!(
            compare::<NAV_MAX_NODES>(&bytes, (-10_000, -10_000), points[0], BikeType::Road, 0).outcome,
            Outcome::NoPath
        );
        if disconnected {
            let result = compare::<NAV_MAX_NODES>(&bytes, points[0], points[107], BikeType::Road, 0);
            assert_eq!(result.outcome, Outcome::NoPath);
            assert_eq!(result.epsilon, (13, 10));
            assert!(!result.table_full_seen);
        } else {
            for bike in BikeType::ALL {
                for (from, to) in [((502_750, 500_250), (507_750, 500_250)), ((507_750, 500_250), (502_750, 500_250))] {
                    assert!(matches!(compare::<NAV_MAX_NODES>(&bytes, from, to, bike, 0).outcome, Outcome::Done(_)));
                }
            }
            let result = compare::<20>(&bytes, points[0], points[107], BikeType::Road, 0);
            assert_eq!(result.outcome, Outcome::Exhausted);
            assert_eq!(result.epsilon, (3, 1));
            assert!(result.table_full_seen);
            let result = compare::<50>(&bytes, points[0], points[107], BikeType::Road, 0);
            assert!(matches!(result.outcome, Outcome::Done(_)));
            assert_eq!(result.epsilon, (2, 1));
            assert!(result.table_full_seen);
        }
    }
}

#[cfg(feature = "external-fixtures")]
#[cfg_attr(miri, ignore)]
#[test]
fn grimsel_plans_match_at_every_step() {
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let source = SliceSource(&bytes);
    let tables = MapTables::parse(&source).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&source, &tables, &cache);
    let mut points = Vec::new();
    reader.for_each_nav_node(&tables.bbox, &mut [0; 512], |node| points.push((node.lon, node.lat))).unwrap();
    points.sort_unstable();
    points.dedup();
    assert!(!points.is_empty());
    random_pairs(&bytes, &points, 64);
}
