//! Terrain-layer suppression (elevation EL10c, #1096): `MapRenderer::set_terrain_layer(false)` drops
//! every style carrying [`StyleFlags::terrain_layer`] **in the collect pass**, before its geometry is
//! decoded — not by drawing it and painting over.
//!
//! Three things are pinned here, and they are exactly the acceptance the #1097 ride review rests on:
//!
//! 1. **Nothing is decoded.** The scene records every style id the collector's `should_decode` filter
//!    asks about; with the layer hidden, the terrain style is never asked for.
//! 2. **The frame is pixel-identical to a map with no terrain styles at all.** Suppressing the layer
//!    must not perturb one pixel of everything else — same painter order, same widths, same colours.
//!    A "draw then overpaint" implementation would fail this the moment a contour crossed a road.
//! 3. **It flips both ways on the same renderer, with no reset.** The setter is sticky across frames
//!    (that is why it can be a setter at all), so the on-frame after an off-frame is byte-for-byte
//!    the frame a fresh renderer would have drawn.
//!
//! **Provisional.** This whole file goes when #1096's toggle does.

mod common;

use core::cell::RefCell;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::RgbColor;
use heapless::Vec;
use obc_map_scene::{
    BBox, Candidate, CandidateReport, DecodeReport, Feature, FeatureToken, Kind, MapScene, SelectedFeatures, Style,
    StyleFlags,
};
use obc_render::{MapRenderer, Viewport};

use common::Buf;

/// The ordinary feature: a red polygon covering the middle of the view.
const ROAD_TOKEN: FeatureToken = FeatureToken::from_source_words([1, 0, 0]);
const ROAD_POINTS: [(i32, i32); 4] = [(-40, -40), (40, -40), (40, 40), (-40, 40)];
const ROAD_BOUNDS: BBox = BBox { min_lon: -40, min_lat: -40, max_lon: 40, max_lat: 40 };
const ROAD: Style =
    Style { id: 7, z_index: 3, color: 0xF800, weight: 1, priority: 1, flags: StyleFlags::NONE, color2: None };

/// The terrain feature: a blue line straight **across** the polygon, at a higher `z` so it paints on
/// top. Overlapping on purpose — a suppression that merely repainted the contour in the backdrop
/// colour would leave a scar through the polygon and fail the pixel-identity check below.
const CONTOUR_TOKEN: FeatureToken = FeatureToken::from_source_words([2, 0, 0]);
const CONTOUR_POINTS: [(i32, i32); 2] = [(-60, 0), (60, 0)];
const CONTOUR_BOUNDS: BBox = BBox { min_lon: -60, min_lat: 0, max_lon: 60, max_lat: 0 };
/// `weight 1`, fixed-width + terrain-layer — the shipped E3 contour style's flag pair (#1095).
const CONTOUR: Style = Style {
    id: 8,
    z_index: 9,
    color: 0x001F,
    weight: 1,
    priority: 2,
    flags: StyleFlags::new(false, true, true),
    color2: None,
};

const ROAD_RINGS: [usize; 1] = [4];
const CONTOUR_RINGS: [usize; 1] = [2];

const RED: Rgb888 = Rgb888::new(255, 0, 0);
const BLUE: Rgb888 = Rgb888::new(0, 0, 255);

/// A two-feature scene. `with_contour = false` builds the **contour-free map** the suppressed frame
/// must reproduce exactly: the terrain style simply isn't in the table and its feature isn't in the
/// data, i.e. what a bake with contours turned off would produce.
struct Scene {
    with_contour: bool,
    /// What the collector's `should_decode` filter answered, per style id offered, in order — the
    /// record that proves suppression happened *before* the decode and not after the draw.
    filtered: RefCell<std::vec::Vec<(u8, bool)>>,
}

impl Scene {
    fn new(with_contour: bool) -> Self {
        Scene { with_contour, filtered: RefCell::new(std::vec::Vec::new()) }
    }

    /// The filter's verdict on `id` this render, or `None` if it was never offered.
    fn verdict(&self, id: u8) -> Option<bool> {
        self.filtered.borrow().iter().find(|&&(sid, _)| sid == id).map(|&(_, ok)| ok)
    }
}

impl MapScene for Scene {
    fn lod_count(&self) -> usize {
        1
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        0
    }

    fn style(&self, id: u8) -> Option<&Style> {
        match id {
            7 => Some(&ROAD),
            8 if self.with_contour => Some(&CONTOUR),
            _ => None,
        }
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        // A real source consults the filter per feature record and skips the geometry bytes of a
        // rejected one without decoding them (`Reader::decode_chunk_into`); mirror that, and record
        // what was asked so the test can prove the contour was never decoded.
        for (id, kind, pts, rings, bounds) in [
            (ROAD.id, Kind::Polygon, &ROAD_POINTS[..], &ROAD_RINGS[..], ROAD_BOUNDS),
            (CONTOUR.id, Kind::Line, &CONTOUR_POINTS[..], &CONTOUR_RINGS[..], CONTOUR_BOUNDS),
        ] {
            if id == CONTOUR.id && !self.with_contour {
                continue;
            }
            let wanted = should_decode(id);
            self.filtered.borrow_mut().push((id, wanted));
            if !wanted {
                continue;
            }
            points.clear();
            ring_lens.clear();
            points.extend_from_slice(pts).unwrap();
            ring_lens.extend_from_slice(rings).unwrap();
            let token = if id == ROAD.id { ROAD_TOKEN } else { CONTOUR_TOKEN };
            visit(Candidate { token, feature: Feature::new(id, kind, points, ring_lens, bounds) });
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
            let Some(token) = selected.token(i) else { continue };
            let (id, kind, pts, rings, bounds) = if token == ROAD_TOKEN {
                (ROAD.id, Kind::Polygon, &ROAD_POINTS[..], &ROAD_RINGS[..], ROAD_BOUNDS)
            } else {
                (CONTOUR.id, Kind::Line, &CONTOUR_POINTS[..], &CONTOUR_RINGS[..], CONTOUR_BOUNDS)
            };
            points.clear();
            ring_lens.clear();
            points.extend_from_slice(pts).unwrap();
            ring_lens.extend_from_slice(rings).unwrap();
            let _ = selected.decoded(i, Feature::new(id, kind, points, ring_lens, bounds));
        }
        DecodeReport { chunks_refetched: 1, ..Default::default() }
    }
}

fn viewport() -> Viewport {
    Viewport::new(120.0, 120.0, 0, 0, 1.0)
}

/// Render `scene` into a fresh buffer with the terrain layer shown or hidden.
fn render(scene: &Scene, terrain: bool) -> (Buf, usize) {
    let mut buf = Buf::new(120, 120);
    let mut renderer = MapRenderer::new();
    renderer.set_terrain_layer(terrain);
    let stats = renderer.render(&mut buf, scene, &viewport(), Rgb888::BLACK, |c| {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    (buf, stats.features_drawn)
}

/// The default renderer draws the terrain layer: nothing has to be switched on to see contours.
#[test]
fn terrain_layer_is_drawn_by_default() {
    let scene = Scene::new(true);
    let mut buf = Buf::new(120, 120);
    // Deliberately *no* `set_terrain_layer` call — the fresh-renderer state is the shown state.
    let stats = MapRenderer::new().render(&mut buf, &scene, &viewport(), Rgb888::BLACK, |c| {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    assert_eq!(stats.features_drawn, 2, "road + contour");
    assert!(buf.count(BLUE) > 0, "the contour is on screen");
    assert!(buf.count(RED) > 0, "and so is the road under it");
    assert_eq!(scene.verdict(CONTOUR.id), Some(true), "the terrain style passes the decode filter");
}

/// Hiding the layer skips the terrain style **at the decode filter** — the collector never asks the
/// source for that geometry — and the ordinary feature is untouched.
#[test]
fn hidden_terrain_is_never_decoded() {
    let scene = Scene::new(true);
    let (buf, drawn) = render(&scene, false);
    assert_eq!(drawn, 1, "only the road survives");
    assert_eq!(buf.count(BLUE), 0, "not one contour pixel");
    assert!(buf.count(RED) > 0, "the road is still drawn");
    // The source *did* offer the contour — the collector's visible-style mask is what refused it, so
    // the geometry bytes were skipped rather than decoded, ranked, drawn and painted over.
    assert_eq!(scene.verdict(CONTOUR.id), Some(false), "the terrain style was offered and the decode filter said no");
    assert_eq!(scene.verdict(ROAD.id), Some(true), "everything else still decodes");
}

/// **The pixel-identity proof.** A frame of the contour-carrying map with the layer hidden is
/// byte-for-byte the frame of a map that never had contour styles at all. Same claim as the grimsel
/// A/B in the PR, at unit scale.
#[test]
fn hidden_terrain_is_pixel_identical_to_a_contour_free_map() {
    let (suppressed, drawn_suppressed) = render(&Scene::new(true), false);
    let (contour_free, drawn_free) = render(&Scene::new(false), true);
    assert_eq!(drawn_suppressed, drawn_free, "the same features are drawn");
    assert_eq!(suppressed.px, contour_free.px, "suppressing the layer perturbs no other pixel");
}

/// A map with **no** terrain styles is unaffected by the toggle in either position — the committed
/// pre-contour fixtures (and every already-baked map) must render identically whichever way the
/// rider leaves the switch.
#[test]
fn a_map_without_terrain_styles_ignores_the_toggle() {
    let (on, _) = render(&Scene::new(false), true);
    let (off, _) = render(&Scene::new(false), false);
    assert_eq!(on.px, off.px, "no terrain styles ⇒ the toggle is a visual no-op");
}

/// The setter is sticky and reversible on one long-lived renderer — the on-glass requirement: flip
/// it and the next frame changes, with no reboot and no renderer reset.
#[test]
fn the_toggle_flips_both_ways_on_one_renderer() {
    let scene = Scene::new(true);
    let color = |c: u16| {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    };
    let mut renderer = MapRenderer::new();

    let mut on1 = Buf::new(120, 120);
    renderer.render(&mut on1, &scene, &viewport(), Rgb888::BLACK, color);
    assert!(on1.count(BLUE) > 0);

    let mut off = Buf::new(120, 120);
    renderer.set_terrain_layer(false);
    renderer.render(&mut off, &scene, &viewport(), Rgb888::BLACK, color);
    assert_eq!(off.count(BLUE), 0, "the very next frame drops the layer");

    let mut on2 = Buf::new(120, 120);
    renderer.set_terrain_layer(true);
    renderer.render(&mut on2, &scene, &viewport(), Rgb888::BLACK, color);
    assert_eq!(on2.px, on1.px, "and switching back restores the first frame exactly");
}
