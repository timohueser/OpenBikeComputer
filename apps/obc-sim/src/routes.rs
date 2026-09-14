//! GPX and route-file import boundaries. Runtime routes belong to the shared card.

use obc_formats::io::SliceSource;
use obc_host_core::{FlatRouteStore, VecSink};
use obc_route::{gpx_to_obcr, RouteStats};
use std::path::Path;

pub fn convert_gpx(path: &Path) -> Result<(Vec<u8>, RouteStats), String> {
    let gpx = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("route");
    let mut sink = VecSink::default();
    let stats = gpx_to_obcr(&SliceSource(&gpx), name, &mut sink)
        .map_err(|error| format!("convert {}: {error:?}", path.display()))?;
    Ok((sink.bytes().to_vec(), stats))
}

pub fn import_gpx(store: &mut FlatRouteStore, path: &Path) -> Result<RouteStats, String> {
    let (bytes, stats) = convert_gpx(path)?;
    store.import(&bytes).map_err(|error| error.to_string())?;
    Ok(stats)
}

pub fn export_gpx(path: &Path, directory: &Path) -> Result<RouteStats, String> {
    let (bytes, stats) = convert_gpx(path)?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let name = path.file_stem().unwrap_or_default();
    let output = directory.join(name).with_extension("obcr");
    std::fs::write(output, bytes).map_err(|error| error.to_string())?;
    Ok(stats)
}

pub fn seed_retention(
    store: &mut FlatRouteStore,
    id: u64,
    retention: obc_app::Retention,
    utc: u32,
) -> Result<(), String> {
    use obc_host_core::RouteRepository;
    let scope = store.refresh_metadata().map_err(|error| format!("{error:?}"))?;
    let mut tokens = obc_app::device_core::TokenSource::<obc_app::device_core::RetentionTag>::new();
    store
        .write_metadata(obc_app::retention::RetentionEffect::WriteRouteMetadata {
            token: tokens.issue(),
            scope,
            id,
            meta: obc_app::RouteRetentionMeta::new(retention, utc),
        })
        .map_err(|error| format!("{error:?}"))?;
    store.refresh_metadata().map_err(|error| format!("{error:?}"))?;
    Ok(())
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
        assert_eq!(super::convert_gpx(source).unwrap().0, expected);
    }
}
