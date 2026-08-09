//! WX10 paint-order and byte-identity integration tests for the rain overlay hook
//! ([`RenderScratch::render_rain_timed`]): rain draws **over** the ground fills and **under** the
//! road band, and a frame without rain (no source, or an all-dry / no-data frame) is byte-identical
//! to the plain render path.

mod common;

use embedded_graphics::pixelcolor::Rgb888;
use heapless::Vec;
use obc_map_scene::{
    BBox, Candidate, CandidateReport, DecodeReport, Feature, FeatureToken, Kind, MapScene, SelectedFeatures, Style,
    StyleFlags,
};
use obc_render::{
    rain_style, NoopClock, RainGrid, RainOverlaySource, RenderConfig, RenderScratch, Viewport, RAIN_BELOW_Z,
    RAIN_TILE_CELLS,
};

use common::Buf;

/// A ground fill covering the whole tiny map, below the rain boundary.
const GROUND: Style =
    Style { id: 1, z_index: 1, color: 0xFFF5, weight: 1, priority: 1, flags: StyleFlags::NONE, color2: None };
/// A road line across the middle, in the road band above the rain boundary.
const ROAD: Style =
    Style { id: 2, z_index: 30, color: 0xFAA0, weight: 3, priority: 2, flags: StyleFlags::NONE, color2: None };
const _: () = assert!(GROUND.z_index < RAIN_BELOW_Z && ROAD.z_index >= RAIN_BELOW_Z);

const GROUND_TOKEN: FeatureToken = FeatureToken::from_source_words([1, 0, 0]);
const ROAD_TOKEN: FeatureToken = FeatureToken::from_source_words([2, 0, 0]);
const GROUND_POINTS: [(i32, i32); 4] = [(-4000, -4000), (4000, -4000), (4000, 4000), (-4000, 4000)];
const ROAD_POINTS: [(i32, i32); 2] = [(-4000, 0), (4000, 0)];
const BOUNDS: BBox = BBox { min_lon: -4000, min_lat: -4000, max_lon: 4000, max_lat: 4000 };

/// A ground polygon plus one road line — enough scene to pin the paint order around the hook.
struct RoadScene;

impl MapScene for RoadScene {
    fn lod_count(&self) -> usize {
        1
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        0
    }

    fn style(&self, id: u8) -> Option<&Style> {
        match id {
            1 => Some(&GROUND),
            2 => Some(&ROAD),
            _ => None,
        }
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        if !BOUNDS.intersects(view) {
            return CandidateReport { chunks_visited: 1, ..Default::default() };
        }
        for (token, style, kind, feature_points) in [
            (GROUND_TOKEN, &GROUND, Kind::Polygon, &GROUND_POINTS[..]),
            (ROAD_TOKEN, &ROAD, Kind::Line, &ROAD_POINTS[..]),
        ] {
            if should_decode(style.id) {
                points.clear();
                ring_lens.clear();
                points.extend_from_slice(feature_points).unwrap();
                ring_lens.push(feature_points.len()).unwrap();
                visit(Candidate { token, feature: Feature::new(style.id, kind, points, ring_lens, BOUNDS) });
            }
        }
        CandidateReport { chunks_visited: 1, ..Default::default() }
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        for i in 0..selected.len() {
            if !selected.is_pending(i) {
                continue;
            }
            let Some(token) = selected.token(i) else { continue };
            let (style, kind, feature_points) = if token == GROUND_TOKEN {
                (&GROUND, Kind::Polygon, &GROUND_POINTS[..])
            } else {
                (&ROAD, Kind::Line, &ROAD_POINTS[..])
            };
            points.clear();
            ring_lens.clear();
            points.extend_from_slice(feature_points).unwrap();
            ring_lens.push(feature_points.len()).unwrap();
            let _ = selected.decoded(i, Feature::new(style.id, kind, points, ring_lens, BOUNDS));
        }
        DecodeReport { chunks_refetched: 1, ..Default::default() }
    }
}

/// A uniform-intensity source over the whole scene bounds.
struct UniformRain(u8);

impl RainOverlaySource for UniformRain {
    fn grid(&self) -> Option<RainGrid> {
        Some(RainGrid {
            west_udeg: -4000,
            south_udeg: -4000,
            east_udeg: 4000,
            north_udeg: 4000,
            width_cells: 16,
            height_cells: 16,
        })
    }

    fn tile(&mut self, _tile_index: u32, out: &mut [u8; RAIN_TILE_CELLS]) -> bool {
        out.fill(self.0);
        true
    }
}

fn rgb888(c: u16) -> Rgb888 {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    Rgb888::new((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
}

fn render(rain: Option<&mut dyn RainOverlaySource>) -> Buf {
    let mut buf = Buf::new(64, 64);
    let mut scratch = RenderScratch::new();
    let vp = Viewport::new(64.0, 64.0, 0, 0, 0.008);
    scratch.render_rain_timed(
        &mut buf,
        &RoadScene,
        &vp,
        rgb888(0x0000),
        RenderConfig::default(),
        rain,
        rgb888,
        &NoopClock,
    );
    buf
}

#[test]
fn rain_paints_over_ground_and_under_roads() {
    let no_rain = render(None);
    let mut heavy = UniformRain(12); // torrential: coverage 16 ⇒ every in-grid pixel painted
    let rained = render(Some(&mut heavy));

    let road = rgb888(ROAD.color);
    let rain_color = rgb888(rain_style(12).0);

    // The road band survives byte-for-byte: every road pixel of the dry frame is still a road
    // pixel under torrential rain — rain draws below roads, never over them.
    let mut road_px = 0usize;
    for y in 0..64 {
        for x in 0..64 {
            if no_rain.get(x, y) == road {
                road_px += 1;
                assert_eq!(rained.get(x, y), road, "road pixel ({x},{y}) painted over by rain");
            }
        }
    }
    assert!(road_px > 0, "the scene actually drew a road");

    // And the ground did get rained on — at full coverage, every former ground pixel inside the
    // grid is now the rain color.
    let ground = rgb888(GROUND.color);
    assert!(no_rain.count(ground) > 0);
    assert_eq!(rained.count(ground), 0, "torrential coverage covers all ground pixels");
    assert!(rained.count(rain_color) > 0);
}

#[test]
fn light_rain_dithers_transparency_not_values() {
    let no_rain = render(None);
    let mut light = UniformRain(1); // coverage 4/16
    let rained = render(Some(&mut light));

    let rain_color = rgb888(rain_style(1).0);
    let (mut rain_px, mut kept_px, mut other_px) = (0usize, 0usize, 0usize);
    for y in 0..64 {
        for x in 0..64 {
            let (before, after) = (no_rain.get(x, y), rained.get(x, y));
            if after == before {
                kept_px += 1;
            } else if after == rain_color {
                rain_px += 1;
            } else {
                other_px += 1;
            }
        }
    }
    // Ordered dithering as transparency: changed pixels take exactly the band color, every other
    // pixel keeps the basemap, and nothing is blended into a third value.
    assert_eq!(other_px, 0, "dither must never mix colors");
    assert!(rain_px > 0 && kept_px > rain_px, "light rain leaves most of the basemap visible");
    // Coverage 4/16 paints exactly a quarter of the pixels of any fully-in-grid 4×4 block.
    let block_hits = (0..4)
        .flat_map(|dy| (0..4).map(move |dx| (16 + dx, 16 + dy)))
        .filter(|&(x, y)| rained.get(x, y) == rain_color)
        .count();
    assert_eq!(block_hits, 4, "coverage 4/16 paints one quarter of a 4×4 Bayer block");
}

#[test]
fn absent_dry_and_nodata_rain_are_byte_identical_to_the_plain_path() {
    let plain = render(None);
    for code in [0u8, 15] {
        let mut source = UniformRain(code);
        let rained = render(Some(&mut source));
        assert_eq!(plain.px, rained.px, "intensity {code} must not change a single byte");
    }
    // And the hookless legacy entry stays bit-equal to the hook with `None`.
    let mut buf = Buf::new(64, 64);
    let mut scratch = RenderScratch::new();
    let vp = Viewport::new(64.0, 64.0, 0, 0, 0.008);
    scratch.render_timed(&mut buf, &RoadScene, &vp, rgb888(0x0000), RenderConfig::default(), rgb888, &NoopClock);
    assert_eq!(plain.px, buf.px);
}
