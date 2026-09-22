//! Bounded host measurement of the shipping planner. See dev/navigation/README.md.
use std::{cell::Cell, time::Instant};

use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{NavPhase, NavPlanner, NavScratch};

struct Counted<'a> {
    bytes: &'a [u8],
    calls: Cell<u64>,
    read_bytes: Cell<u64>,
}
impl ByteSource for Counted<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.calls.set(self.calls.get() + 1);
        self.read_bytes.set(self.read_bytes.get() + buf.len() as u64);
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
        self.0.get_mut(offset as usize..offset as usize + bytes.len()).ok_or(Error::BadOffset)?.copy_from_slice(bytes);
        Ok(())
    }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 8 || args.len() == 9,
        "nav_cost MAP FROM_LON FROM_LAT TO_LON TO_LAT PROFILE OUTPUT [ORIGINAL_OBCR]"
    );
    let bytes = std::fs::read(&args[1]).unwrap();
    let src = Counted { bytes: &bytes, calls: Cell::new(0), read_bytes: Cell::new(0) };
    let tables = MapTables::parse(&src).unwrap();
    let map_cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &map_cache);
    let from: (i32, i32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
    let to = (args[4].parse().unwrap(), args[5].parse().unwrap());
    let profile = obc_route::BikeType::from_u8(args[6].parse().unwrap()).expect("bike type 0..=3");
    let mut planner = if let Some(path) = args.get(8) {
        let original = std::fs::read(path).unwrap();
        let source = SliceSource(&original);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = obc_route::RouteReader::new(&index, &source);
        NavPlanner::new_detour(
            from,
            to,
            "NG cost",
            profile,
            obc_route::Corridor::build(&route, 0, index.total_distance_m),
        )
    } else {
        NavPlanner::new(from, to, "NG cost", profile)
    };
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut cache = NavTileCache::new();
    let mut sink = Sink::default();
    let mut phase_ns = [0u128; 3];
    let mut phase_steps = [0u32; 3];
    let mut phase_reads = [0u64; 3];
    let mut phase_bytes = [0u64; 3];
    let mut max_ns = 0;
    let mut retries = 0;
    let mut epsilon = planner.epsilon_used();
    src.calls.set(0);
    src.read_bytes.set(0);
    let start = Instant::now();
    let outcome = loop {
        let phase = match planner.phase() {
            NavPhase::Snap => 0,
            NavPhase::Search => 1,
            NavPhase::Emit => 2,
            NavPhase::Done => unreachable!(),
        };
        let reads = src.calls.get();
        let read_bytes = src.read_bytes.get();
        let step_start = Instant::now();
        let step = planner.step(&reader, &mut scratch, &mut cache, &mut obc_route::NullElevation, &mut sink);
        let elapsed = step_start.elapsed().as_nanos();
        phase_ns[phase] += elapsed;
        phase_steps[phase] += 1;
        phase_reads[phase] += src.calls.get() - reads;
        phase_bytes[phase] += src.read_bytes.get() - read_bytes;
        max_ns = max_ns.max(elapsed);
        if planner.epsilon_used() != epsilon {
            retries += 1;
            epsilon = planner.epsilon_used();
        }
        assert!(phase_steps.iter().sum::<u32>() < 100_000, "planner did not finish");
        match step {
            obc_route::Step::Running => (),
            obc_route::Step::Done(stats) => break format!("ok:{}", stats.total_distance_m),
            obc_route::Step::Failed(error) => break format!("{error:?}"),
        }
    };
    let total_ns = start.elapsed().as_nanos();
    if outcome.starts_with("ok:") {
        let index = obc_route::RouteIndex::read(&SliceSource(&sink.0)).unwrap();
        assert!(index.total_distance_m > 0);
        std::fs::write(&args[7], &sink.0).unwrap();
    }
    let stats = cache.stats();
    let measured_calls = src.calls.get();
    let measured_bytes = src.read_bytes.get();
    // Observe the input projection after measurement; this cannot warm the timed plan.
    let view = obc_map_scene::BBox {
        min_lon: from.0 - 10_000,
        min_lat: from.1 - 10_000,
        max_lon: from.0 + 10_000,
        max_lat: from.1 + 10_000,
    };
    let candidate = reader.nearest_nav_edge_candidate_cached(&view, &mut cache, from, 1000.0).unwrap();
    let interior = candidate.is_some_and(|c| c.position.coord != c.a_coord && c.position.coord != c.b_coord);
    println!("{{\"from_interior\":{interior}}}");
    #[cfg(feature = "nav-metrics")]
    println!("{{\"decoded_junctions\":{},\"quadtree_visits\":{}}}", stats.decoded_junctions, stats.quadtree_visits);
    println!("{{\"outcome\":\"{outcome}\",\"total_ns\":{total_ns},\"phase_ns\":{phase_ns:?},\"phase_steps\":{phase_steps:?},\"phase_reads\":{phase_reads:?},\"phase_bytes\":{phase_bytes:?},\"max_step_ns\":{max_ns},\"settles\":{},\"retries\":{retries},\"graph_hits\":{},\"graph_fills\":{},\"index_hits\":{},\"index_fills\":{},\"source_calls\":{},\"source_bytes\":{},\"workspace_bytes\":{},\"output_bytes\":{}}}", planner.settles(), stats.hits, stats.misses, stats.index_hits, stats.index_misses, measured_calls, measured_bytes, std::mem::size_of_val(&scratch)+std::mem::size_of_val(&cache)+std::mem::size_of_val(&planner), sink.0.len());
}
