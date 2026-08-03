//! Elevation labels on index contours (#1106): who gets a number, who never does, and what the
//! pass costs a frame that has none.
//!
//! The scenes here are built so the framebuffer answers every question on its own. Nothing in the
//! map is painted white and the frame clears to black, so the only white pixels on screen are the
//! label *pills*. Counting them is therefore an exact measure of the label pass's footprint, which
//! is what turns "the labels do not overlap" from an internal invariant into an observable one:
//! every label prints the same four digits, so twelve disjoint labels paint exactly twelve times one
//! label's pill pixels.
//!
//! What is pinned:
//!
//! 1. **Index contours are labelled, and nothing else is** — the major contour beside them and the
//!    index contour whose feature states no level both draw as bare lines.
//! 2. **A frame with no label candidate is byte-identical** to the same frame drawn by a renderer
//!    that had never heard of labels — the pass's whole cost there is one `is_empty` branch.
//! 3. **Terrain suppression takes the labels with it, for free** (#1096): suppressed styles never
//!    leave the collect pass, so they never become candidates.
//! 4. **The cap and the collision rule hold** under crowding, and
//! 5. **a label that would touch the viewport edge is skipped**, not drawn clipped mid-glyph.

mod common;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::RgbColor;
use heapless::Vec as HVec;
use obc_map_scene::{
    BBox, Candidate, CandidateReport, DecodeReport, Feature, FeatureToken, Kind, MapScene, SelectedFeatures, Style,
    StyleFlags,
};
use obc_render::{MapRenderer, RenderStats, Viewport, MAX_CONTOUR_LABELS};

use common::Buf;

/// A backdrop style with no feature of its own — present only so the scene is a plausible map.
const BACKDROP: Style =
    Style { id: 1, z_index: -10, color: 0x64DD, weight: 0, priority: 1, flags: StyleFlags::NONE, color2: None };

/// The shipped index-contour style's flag set: fixed width + terrain layer + contour index.
const INDEX: Style = Style {
    id: 8,
    z_index: 9,
    color: 0x001F,
    weight: 1,
    priority: 2,
    flags: StyleFlags::new(false, true, true, true),
    color2: None,
};

/// The same style without the index bit — the 100 m majors, which a map never labels.
const MAJOR: Style = Style {
    id: 9,
    z_index: 9,
    color: 0x001F,
    weight: 1,
    priority: 2,
    flags: StyleFlags::new(false, true, true, false),
    color2: None,
};

const WHITE: Rgb888 = Rgb888::new(255, 255, 255);
const BLACK: Rgb888 = Rgb888::new(0, 0, 0);
const BLUE: Rgb888 = Rgb888::new(0, 0, 255);

/// One contour in a test scene: a polyline of a style, optionally stating its elevation.
struct Feat {
    style: u8,
    pts: std::vec::Vec<(i32, i32)>,
    level: Option<i16>,
}

impl Feat {
    fn bbox(&self) -> BBox {
        let mut b = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
        for &(lon, lat) in &self.pts {
            b.min_lon = b.min_lon.min(lon);
            b.min_lat = b.min_lat.min(lat);
            b.max_lon = b.max_lon.max(lon);
            b.max_lat = b.max_lat.max(lat);
        }
        b
    }
}

struct Scene(std::vec::Vec<Feat>);

impl Scene {
    /// Publish feature `i` into the caller's decode scratch and hand it to `visit`.
    fn publish<const P: usize, const R: usize, T>(
        &self,
        i: usize,
        points: &mut HVec<(i32, i32), P>,
        ring_lens: &mut HVec<usize, R>,
        visit: impl FnOnce(Feature<'_>) -> T,
    ) -> T {
        let f = &self.0[i];
        points.clear();
        ring_lens.clear();
        points.extend_from_slice(&f.pts).unwrap();
        ring_lens.push(f.pts.len()).unwrap();
        visit(Feature::new(f.style, Kind::Line, points, ring_lens, f.bbox()).with_level(f.level))
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
            1 => Some(&BACKDROP),
            8 => Some(&INDEX),
            9 => Some(&MAJOR),
            _ => None,
        }
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut HVec<(i32, i32), P>,
        ring_lens: &mut HVec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        for i in 0..self.0.len() {
            if !should_decode(self.0[i].style) {
                continue;
            }
            self.publish(i, points, ring_lens, |feature| {
                visit(Candidate { token: FeatureToken::from_source_words([i as u16, 0, 0]), feature })
            });
        }
        CandidateReport { chunks_visited: 1, ..Default::default() }
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut HVec<(i32, i32), P>,
        ring_lens: &mut HVec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        for k in 0..selected.len() {
            let Some(token) = selected.token(k) else { continue };
            let i = token.source_words()[0] as usize;
            self.publish(i, points, ring_lens, |feature| selected.decoded(k, feature));
        }
        DecodeReport { chunks_refetched: 1, ..Default::default() }
    }
}

/// A horizontal contour of `style` at latitude `lat`, spanning `lon` `-600..=600` in 50 µdeg steps —
/// long enough that the ~180 µdeg label cadence offers several anchors inside the view whatever the
/// feature's phase.
fn contour(style: u8, lat: i32, level: Option<i16>) -> Feat {
    Feat { style, pts: (-600..=600).step_by(50).map(|lon| (lon, lat)).collect(), level }
}

/// A 320×320 view centred on the equator at 1 px per µdeg (≈ 0.11 m/px), so screen x = lon + 160 and
/// screen y = 160 − lat. Wide enough that a 54-px label always clears the edge somewhere along a
/// full-width contour, whatever its cadence phase.
fn viewport(size: f32) -> Viewport {
    Viewport::new(size, size, 0, 0, 1.0)
}

fn render_sized(scene: &Scene, terrain: bool, size: f32) -> (Buf, RenderStats) {
    let mut buf = Buf::new(size as i32, size as i32);
    let mut renderer = MapRenderer::new();
    renderer.set_terrain_layer(terrain);
    // White pill (the untouched default too), and the contour's own blue as the ink so the digits
    // are countable against the map's own line. The app ships parchment-on-ink.
    renderer.set_label_colors(0xFFFF, INDEX.color);
    let stats = renderer.render(&mut buf, scene, &viewport(size), Rgb888::BLACK, |c| {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    (buf, stats)
}

fn render(scene: &Scene) -> (Buf, RenderStats) {
    render_sized(scene, true, 320.0)
}

/// Blue pixels off the contour's own rows — i.e. the digits. A `weight 1` fixed-width contour paints
/// exactly its own row, so anything blue elsewhere is glyph ink.
fn ink_off_the_line(buf: &Buf, line_rows: &[i32]) -> usize {
    let mut n = 0;
    for y in 0..buf.h {
        if line_rows.contains(&y) {
            continue;
        }
        for x in 0..buf.w {
            if buf.get(x, y) == BLUE {
                n += 1;
            }
        }
    }
    n
}

/// An index contour that states its elevation gets a number: a knockout pill cut through the line,
/// with the level printed in the contour's own ink.
#[test]
fn index_contours_are_labelled() {
    let (buf, stats) = render(&Scene(std::vec![contour(INDEX.id, 0, Some(2500))]));
    assert!(stats.contour_labels >= 1, "the contour is labelled, got {}", stats.contour_labels);
    assert!(buf.count(WHITE) > 0, "the knockout pill is painted in the backdrop colour");
    // The pill sits *on* the line: the row the contour draws on carries backdrop pixels now.
    let (_, y0, _, y1) = buf.bbox(WHITE).expect("a pill");
    assert!(y0 < 160 && y1 > 160, "the pill straddles the contour at y = 160, got rows {y0}..{y1}");
    assert!(ink_off_the_line(&buf, &[160]) > 50, "the digits are drawn above and below the line");
}

/// **Index contours only.** The 100 m majors carry no index bit, so they stay anonymous — the
/// renderer decides by what the style *is* (OBCM §2 bit 6), never by sniffing z-index or guessing a
/// level modulus the file does not carry.
#[test]
fn major_contours_are_never_labelled() {
    let (buf, stats) = render(&Scene(std::vec![contour(MAJOR.id, 0, Some(2500))]));
    assert_eq!(stats.contour_labels, 0, "a major contour is a texture, not a label");
    assert_eq!(buf.count(WHITE), 0, "no pill anywhere");
    assert_eq!(ink_off_the_line(&buf, &[160]), 0, "and no digits");
}

/// **The pixel-identity proof.** An index contour whose feature states no level is not a candidate,
/// so the label pass returns on its `is_empty` branch — and the frame is byte-for-byte the frame of
/// the same line drawn as an unlabellable major, i.e. what the pre-#1106 renderer drew.
#[test]
fn a_frame_with_no_candidate_is_pixel_identical() {
    let (no_level, stats) = render(&Scene(std::vec![contour(INDEX.id, 0, None)]));
    let (major, _) = render(&Scene(std::vec![contour(MAJOR.id, 0, Some(2500))]));
    assert_eq!(stats.contour_labels, 0, "no level, no label");
    assert_eq!(no_level.px, major.px, "the label pass perturbs no pixel when it has no candidate");
}

/// The #1096 terrain toggle takes the labels with it and costs nothing to do so: a suppressed style
/// never leaves the collect pass, so it never becomes a label candidate. The suppressed frame is
/// byte-identical to a map that has no contour at all.
#[test]
fn suppressing_the_terrain_layer_drops_the_labels() {
    let scene = Scene(std::vec![contour(INDEX.id, 0, Some(2500))]);
    let (shown, shown_stats) = render_sized(&scene, true, 320.0);
    let (hidden, hidden_stats) = render_sized(&scene, false, 320.0);
    let (empty, _) = render_sized(&Scene(std::vec![]), true, 320.0);
    assert!(shown_stats.contour_labels >= 1, "shown: labelled");
    assert_eq!(hidden_stats.contour_labels, 0, "hidden: not one label");
    assert_eq!(hidden.px, empty.px, "hiding the layer leaves the frame the contour-free map draws");
    assert_ne!(shown.px, hidden.px, "…and the two frames really do differ");
}

/// **Cap + collisions.** Thirty index contours 8 µdeg apart, all at the same elevation, all offering
/// anchors across the whole view: the frame draws exactly [`MAX_CONTOUR_LABELS`], and their pills
/// paint exactly `MAX_CONTOUR_LABELS ×` one label's pill pixels — which they can only do if no two
/// of them share a pixel.
#[test]
fn crowded_contours_hit_the_cap_without_overlapping() {
    let one = render(&Scene(std::vec![contour(INDEX.id, 0, Some(2500))]));
    assert_eq!(one.1.contour_labels, 1, "the single-contour reference draws one label");
    let pill_px = one.0.count(WHITE);

    let crowded: std::vec::Vec<Feat> =
        (0..30).map(|k| contour(INDEX.id, -120 + k * 8, Some(2500))).collect::<std::vec::Vec<_>>();
    let (buf, stats) = render(&Scene(crowded));
    assert_eq!(stats.contour_labels, MAX_CONTOUR_LABELS, "the cap is the cap, however dense the terrain");
    assert_eq!(
        buf.count(WHITE),
        MAX_CONTOUR_LABELS * pill_px,
        "twelve labels paint twelve disjoint pills — any overlap would repaint a neighbour"
    );
}

/// **The screen edge.** A label is skipped, never clipped: in a viewport too small to hold one, the
/// contour draws bare and the frame is identical to the same scene with nothing to label.
#[test]
fn a_label_that_would_clip_the_edge_is_skipped() {
    // 40×40: a four-digit label is 54 px wide, so no anchor on the line can ever clear the margins.
    let (buf, stats) = render_sized(&Scene(std::vec![contour(INDEX.id, 0, Some(2500))]), true, 40.0);
    let (bare, _) = render_sized(&Scene(std::vec![contour(INDEX.id, 0, None)]), true, 40.0);
    assert_eq!(stats.contour_labels, 0, "nowhere to put a label that fits");
    assert_eq!(buf.count(WHITE), 0, "not half a pill, not one clipped glyph");
    assert_eq!(buf.px, bare.px, "the frame is the unlabelled one, exactly");
}

/// The **untouched** renderer's label colours are the device's own paper and ink — white pill, black
/// number. That is what the all-zero [`MapRenderer`] state has to mean (`init_zeroed` writes zeros
/// and the `PARCHMENT`/`INK` pair the app sets quantizes to exactly this pair on the 64-colour
/// panel), so a host that never calls `set_label_colors` still draws a legible label.
#[test]
fn the_default_label_colours_are_paper_and_ink() {
    let scene = Scene(std::vec![contour(INDEX.id, 0, Some(2500))]);
    let mut buf = Buf::new(320, 320);
    // Deliberately no `set_label_colors` call.
    let stats = MapRenderer::new().render(&mut buf, &scene, &viewport(320.0), Rgb888::BLACK, |c| {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    assert!(stats.contour_labels >= 1, "the contour is labelled");
    let (x0, y0, x1, y1) = buf.bbox(WHITE).expect("a white pill");
    let ink = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).filter(|&(x, y)| buf.get(x, y) == BLACK).count();
    assert!(ink > 50, "the digits are drawn in black on the pill, got {ink} px");
}
