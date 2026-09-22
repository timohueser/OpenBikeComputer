use super::VecSink;
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavError, NavScratch};

/// Parse `bytes` and run the router for `bike` with a full-size scratch +
/// fresh tile cache, returning `(result, obcr_bytes, cache_stats)`.
pub fn plan_p(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    bike: obc_route::BikeType,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>, obc_reader::NavCacheStats) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, name, bike, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
    (res, sink.buf, tiles.stats())
}

/// FNV-1a over the emitted bytes — a compact stand-in for pasting a whole OBCR into the test.
pub fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
