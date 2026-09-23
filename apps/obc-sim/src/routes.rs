//! GPX and route-file import boundaries. Runtime routes belong to the shared card.

use obc_host_core::{convert_gpx, FlatRouteStore};
use obc_reader::Reader;
use obc_route::RouteStats;
use std::path::Path;

pub fn import_gpx(
    store: &mut FlatRouteStore,
    path: &Path,
    bike: obc_route::BikeType,
    map: Option<(&Reader, obc_formats::obcr::RouteSourceKey)>,
) -> Result<RouteStats, String> {
    let (bytes, stats) = convert_gpx(path, bike, map)?;
    store.import(&bytes).map_err(|error| error.to_string())?;
    Ok(stats)
}

pub fn export_gpx(
    path: &Path,
    directory: &Path,
    bike: obc_route::BikeType,
    map: Option<(&Reader, obc_formats::obcr::RouteSourceKey)>,
) -> Result<RouteStats, String> {
    let (bytes, stats) = convert_gpx(path, bike, map)?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let name = path.file_stem().unwrap_or_default();
    let output = directory.join(name).with_extension("obcr");
    std::fs::write(output, bytes).map_err(|error| error.to_string())?;
    Ok(stats)
}
pub fn import_copy(store: &mut FlatRouteStore, index: usize, replace: bool) -> Result<u64, String> {
    use obc_formats::io::ByteSource;
    use obc_host_core::RouteRepository;
    let id = *store.ids().get(index).ok_or("route selection is stale")?;
    let source = store.source(id).map_err(|error| format!("{error:?}"))?;
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes).map_err(|error| format!("{error:?}"))?;
    if replace { store.replace(id, &bytes).map(|()| id) } else { store.import(&bytes) }
        .map_err(|error| error.to_string())
}

pub fn elevation_sparkline(store: &FlatRouteStore, id: u64) -> Option<[u8; obc_route::SPARKLINE_BUCKETS]> {
    obc_route::elevation_sparkline(&store.source(id).ok()?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn committed_route_asset_matches_the_gpx_conversion() {
        let source = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx"
        ));
        let expected = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
        assert_eq!(super::convert_gpx(source, obc_route::BikeType::Road, None).unwrap().0, expected);
    }
}
