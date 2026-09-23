//! GPX route input: one conversion to OBCR bytes, attributed against the host's map.

use crate::VecSink;
use obc_formats::io::SliceSource;
use obc_reader::{NavTileCache, Reader};
use obc_route::{gpx_to_obcr_attributed, RouteStats};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// Convert the GPX at `path` into OBCR bytes typed `bike`. With a map, every segment carries the surface and
/// road class the map knows plus the source key that binds the route to that exact map revision;
/// without one the route is unattributed. Filename accents are composed and common typography
/// uses device-font equivalents before the route name is capped.
pub fn convert_gpx(
    path: &Path,
    bike: obc_route::BikeType,
    map: Option<(&Reader, obc_formats::obcr::RouteSourceKey)>,
) -> Result<(Vec<u8>, RouteStats), String> {
    let gpx = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("route");
    let name: String = name
        .nfc()
        .map(|c| match c {
            '\u{2018}'..='\u{201a}' => '\'',
            '\u{201c}'..='\u{201e}' => '"',
            '\u{2010}'..='\u{2014}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{202f}' => ' ',
            _ => c,
        })
        .collect();
    let mut sink = VecSink::default();
    let mut tiles = NavTileCache::new();
    let stats = gpx_to_obcr_attributed(&SliceSource(&gpx), &name, bike, &mut sink, map.map(|(_, key)| key), |a, b| {
        map.map_or(Ok(0), |(reader, _)| obc_route::attribution::attribute_segment(reader, &mut tiles, a, b))
    })
    .map_err(|error| format!("convert {}: {error:?}", path.display()))?;
    Ok((sink.bytes().to_vec(), stats))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn imported_route_names_compose_accents_and_fit_the_device_font() {
        let dir = tempfile::tempdir().unwrap();
        let gpx = br#"<gpx><trk><trkseg>
            <trkpt lat="47.0" lon="8.0"/>
            <trkpt lat="47.001" lon="8.001"/>
            <trkpt lat="47.002" lon="8.002"/>
        </trkseg></trk></gpx>"#;
        for (input, expected) in [
            (
                "Kleine Wasserfa\u{308}lle – Blick auf die Bantry Bay Runde von Hungry Hill",
                "Kleine Wasserfälle - Blick auf die Bantry Bay R",
            ),
            ("Cafe\u{301} d’Annecy — l’e\u{301}te\u{301}", "Café d'Annecy - l'été"),
            ("Grüner Weg", "Grüner Weg"),
        ] {
            let path = dir.path().join(format!("{input}.gpx"));
            std::fs::write(&path, gpx).unwrap();
            let (bytes, _) = convert_gpx(&path, obc_route::BikeType::Road, None).unwrap();
            let route = obc_route::RouteSummary::read(&SliceSource(&bytes)).unwrap();
            assert_eq!(route.name.as_str(), expected);
            assert!(route.name.chars().all(obc_render::glyph_supported));
        }
    }
}
