//! Host repro harness for the on-device nav planner (#501 fault hunt): run the EXACT device plan
//! against a real `.obcm` on the host, with the device-sized table and the stepped path, and
//! report outcome + settles + per-phase step counts + tile-cache stats + snapped endpoints.
//!
//! ```text
//! cargo run --release -p obc-route --example nav_repro -- <map.obcm> <from_lon> <from_lat> [<to_lon> <to_lat>] [--category N]
//! ```
//!
//! **Coordinates are µdeg, LON FIRST** (the OBCM/tuple convention). A Freiburg fix at
//! lat 47.9959° / lon 7.8522° is therefore `7852200 47995900`. When `<to>` is omitted the
//! harness replicates the device flow: nearest-POI row 0 of `--category N` (default 1 = Water,
//! the first menu category) queried from the fix.
//!
//! Runs three variants for differential signal: stepped `NavScratch<768>` (the device),
//! stepped `NavScratch<1536>` (the sim), and one-shot `plan_route` at 768.

use obc_reader::{MapCache, MapTables, NavTileCache, PoiCategory, Reader, SliceSource};
use obc_route::nav::{NavPhase, NavPlanner, NavScratch, Step};
use obc_route::{ground_dist_m, ByteSink, Error};

struct VecSink(Vec<u8>);
impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        if o + b.len() > self.0.len() {
            return Err(Error::BadOffset);
        }
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

fn stepped<const N: usize>(reader: &Reader, from: (i32, i32), to: (i32, i32)) {
    let mut scratch: Box<NavScratch<N>> = unsafe { Box::new_zeroed().assume_init() };
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink(Vec::new());
    let mut planner = NavPlanner::new(from, to, "Repro", 0);
    let mut steps_by_phase = [0u32; 4]; // snap, search, emit, done
    let outcome = loop {
        let phase = planner.phase();
        steps_by_phase[match phase {
            NavPhase::Snap => 0,
            NavPhase::Search => 1,
            NavPhase::Emit => 2,
            NavPhase::Done => 3,
        }] += 1;
        match planner.step(reader, &mut scratch, &mut tiles, &mut sink) {
            Step::Running => {}
            other => break other,
        }
    };
    let ((sid, sc), (gid, gc)) = planner.endpoints();
    let stats = tiles.stats();
    println!("  stepped N={N}:");
    println!("    snap: start id={sid} at ({},{})  goal id={gid} at ({},{})", sc.0, sc.1, gc.0, gc.1);
    println!(
        "    outcome: {:?} | settles={} | steps snap/search/emit = {}/{}/{} | tiles {} hit / {} miss",
        outcome,
        planner.settles(),
        steps_by_phase[0],
        steps_by_phase[1],
        steps_by_phase[2],
        stats.hits,
        stats.misses
    );
    if let Step::Done(s) = outcome {
        println!(
            "    route: len={} m, {} points, {} chunks, {} bytes emitted",
            s.total_distance_m,
            s.point_count,
            s.chunk_count,
            sink.0.len()
        );
    }
}

fn one_shot(reader: &Reader, from: (i32, i32), to: (i32, i32)) {
    let mut scratch: Box<NavScratch<768>> = unsafe { Box::new_zeroed().assume_init() };
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink(Vec::new());
    let res = obc_route::plan_route(reader, from, to, "Repro", 0, &mut scratch, &mut tiles, &mut sink);
    let stats = tiles.stats();
    println!("  one-shot N=768:");
    match res {
        Ok(s) => println!(
            "    outcome: Ok len={} m, {} points | tiles {} hit / {} miss",
            s.total_distance_m, s.point_count, stats.hits, stats.misses
        ),
        Err(e) => println!("    outcome: Err({e:?}) | tiles {} hit / {} miss", stats.hits, stats.misses),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "usage: nav_repro <map.obcm> <from_lon_udeg> <from_lat_udeg> [<to_lon_udeg> <to_lat_udeg>] [--category N]"
        );
        eprintln!(
            "       coordinates are microdegrees, LON FIRST (Freiburg lat 47.9959/lon 7.8522 => 7852200 47995900)"
        );
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("read map");
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid OBCM");
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let from = (args[1].parse::<i32>().unwrap(), args[2].parse::<i32>().unwrap());
    println!("map: {} ({} bytes) | bbox {:?}", args[0], bytes.len(), reader.bbox);
    println!("from (lon,lat) = {from:?}");

    let to = if args.len() >= 5 && !args[3].starts_with("--") {
        (args[3].parse::<i32>().unwrap(), args[4].parse::<i32>().unwrap())
    } else {
        // Device flow: nearest-POI row 0 of the chosen category (default 1 = Water, the first
        // menu category), queried from the fix.
        let cat_n: u8 = args.iter().position(|a| a == "--category").map(|i| args[i + 1].parse().unwrap()).unwrap_or(1);
        let category = *PoiCategory::ALL.get(cat_n as usize - 1).expect("category 1..=6");
        let mut pois: heapless::Vec<obc_reader::Poi, 16> = heapless::Vec::new();
        reader.nearest_pois(category, from, &mut pois).expect("poi query");
        let poi = pois.first().unwrap_or_else(|| panic!("no {category:?} POI near the fix"));
        println!(
            "target: nearest {category:?} row 0 = '{}' at ({},{}), {} m crow",
            poi.name.as_str(),
            poi.lon,
            poi.lat,
            poi.distance_m
        );
        (poi.lon, poi.lat)
    };
    println!("to (lon,lat) = {to:?} | crow = {:.0} m\n", ground_dist_m(from, to));

    stepped::<768>(&reader, from, to);
    stepped::<1536>(&reader, from, to);
    one_shot(&reader, from, to);
}
