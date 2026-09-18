//! Wiring test for the settlement-name overlay in [`App::render_frame`]: a village stored in the
//! map reaches the panel as haloed text, and only inside its scale band.
//!
//! The two maps are the same bytes apart from the category-9 records, and the map chrome is the
//! same in every frame, so the parchment halo pixels a frame gains are the settlement names.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obcm_testkit::{build_poi_map, PoiSpec};

use crate::common::Buf;

const BBOX: (i32, i32, i32, i32) = (7_000_000, 47_000_000, 8_000_000, 48_000_000);
const CAM: (i32, i32) = (7_500_000, 47_500_000);
/// The halo the map's floating text draws under its ink.
const PARCHMENT: u16 = obc_app::screen::palette::PARCHMENT;

/// A map with the given settlement records. `subtype` 23 is `place=village`.
fn map_with(settlements: &[(i32, i32, &str)]) -> Vec<u8> {
    let specs = settlements
        .iter()
        .map(|&(lat, lon, name)| PoiSpec { lat, lon, subtype: 23, name: name.into(), payload: 13 })
        .collect();
    build_poi_map(BBOX, 512, &[(9, specs)])
}

/// Halo pixels in one 240×320 frame of `bytes` at `mpp` metres per pixel.
fn halo_pixels(bytes: &[u8], mpp: f32) -> usize {
    let mut app = App::new(AppState::new(CAM.0, CAM.1, obc_render::zoom_for_mpp(mpp)));
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid obcm");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame(Some(&mut scratch), &mut buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    let (r, g, b) = rgb565_to_rgb888(PARCHMENT);
    buf.count(Rgb888::new(r, g, b))
}

#[test]
fn a_village_name_reaches_the_panel_inside_its_scale_band() {
    let bare = map_with(&[]);
    let named = map_with(&[(CAM.1 + 4_000, CAM.0 + 2_000, "Denzlingen")]);

    // 30 m/px is inside the village band: the name adds haloed text to the frame.
    assert!(
        halo_pixels(&named, 30.0) > halo_pixels(&bare, 30.0),
        "a village inside its band paints its name over the map"
    );
    // 100 m/px is past the village limit, and 1 m/px is below the whole overlay.
    for outside in [100.0, 1.0] {
        assert_eq!(
            halo_pixels(&named, outside),
            halo_pixels(&bare, outside),
            "{outside} m/px is outside the village band, so the map draws the same frame"
        );
    }
}
