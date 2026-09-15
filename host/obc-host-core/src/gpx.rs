//! GPX route input: one conversion to OBCR bytes, attributed against the host's map.

use crate::VecSink;
use obc_formats::io::SliceSource;
use obc_reader::{NavTileCache, Reader};
use obc_route::{gpx_to_obcr_attributed, RouteStats};
use std::path::Path;

/// Convert the GPX at `path` into OBCR bytes. With a map, every segment carries the surface and
/// road class the map knows plus the source key that binds the route to that exact map revision;
/// without one the route is unattributed.
pub fn convert_gpx(
    path: &Path,
    map: Option<(&Reader, obc_formats::obcr::RouteSourceKey)>,
) -> Result<(Vec<u8>, RouteStats), String> {
    let gpx = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("route");
    let mut sink = VecSink::default();
    let mut tiles = NavTileCache::new();
    let stats = gpx_to_obcr_attributed(&SliceSource(&gpx), name, &mut sink, map.map(|(_, key)| key), |a, b| {
        map.map_or(Ok(0), |(reader, _)| obc_route::attribution::attribute_segment(reader, &mut tiles, a, b))
    })
    .map_err(|error| format!("convert {}: {error:?}", path.display()))?;
    Ok((sink.bytes().to_vec(), stats))
}
