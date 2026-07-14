//! Screen-space broad-phase cull coverage for [`Viewport::bbox_may_touch_screen`] (issue #847).
//!
//! The map-space quadtree walk and per-feature AABB test admit everything inside the *enclosing*
//! AABB of the viewport. Heading-up, that AABB has large empty corners the rotated screen rectangle
//! never covers. This second, renderer-owned test rejects a candidate whose projected-corner screen
//! AABB cannot touch the (ink-margin-expanded) display. It must never false-negative (drop a visible
//! feature), so these tests pin: the safe-reject corners, the inclusive-pixel + margin boundaries,
//! and that a max-width cased line whose *centerline* bbox sits just off-screen is still admitted
//! because its ink reaches the panel.

use core::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use obc_map_scene::BBox;
use obc_render::Viewport;

/// The renderer's base-map ink margin (`MAX_LINE_PX 12 + 2*CASING_PX 2 + safety 2`). `pub(crate)` in
/// the crate, so mirrored here as the literal the integration tests exercise; the exact-margin cases
/// below are what prove the 16-px choice covers the widest stroke a feature can paint past its bbox.
const INK_MARGIN: i32 = 16;

/// A tiny (1 µdeg) bbox centred on a single map point — for placing a probe at a precise corner.
fn point_bbox(lon: i32, lat: i32) -> BBox {
    BBox { min_lon: lon, min_lat: lat, max_lon: lon + 1, max_lat: lat + 1 }
}

/// A zero-area bbox at exactly one map point — its four corners project to a single pixel, so the
/// screen AABB is that pixel, for bit-exact inclusive-boundary assertions.
fn px(lon: i32, lat: i32) -> BBox {
    BBox { min_lon: lon, min_lat: lat, max_lon: lon, max_lat: lat }
}

/// A 240×240 north-up viewport at the equator/prime-meridian: `aspect = 1`, `zoom = 1`, camera at
/// screen centre (120, 120), so `to_screen(lon, lat) == (120 + lon, 120 - lat)` — a clean grid for
/// pixel-exact boundary assertions. Inclusive screen pixels are x,y ∈ [0, 239].
fn north_up() -> Viewport {
    Viewport::new(240.0, 240.0, 0, 0, 1.0)
}

#[test]
fn fully_inside_is_admitted() {
    let vp = north_up();
    // A bbox well within the panel projects inside the display rect; admitted at zero margin.
    assert!(vp.bbox_may_touch_screen(&BBox { min_lon: -30, min_lat: -30, max_lon: 30, max_lat: 30 }, 0));
}

#[test]
fn fully_outside_each_side_is_rejected() {
    let vp = north_up();
    // Screen covers lon ∈ [-120, 119] (x = lon+120 ∈ [0, 239]) and lat ∈ [-119, 120]. A bbox wholly
    // past one side by a clear margin must be rejected at margin 0 — one per side.
    let right = BBox { min_lon: 200, min_lat: -5, max_lon: 210, max_lat: 5 }; // x ∈ [320, 330]
    let left = BBox { min_lon: -210, min_lat: -5, max_lon: -200, max_lat: 5 }; // x ∈ [-90, -80]
    let top = BBox { min_lon: -5, min_lat: 200, max_lon: 5, max_lat: 210 }; // y = 120-lat ∈ [-90, -80]
    let bottom = BBox { min_lon: -5, min_lat: -210, max_lon: 5, max_lat: -200 }; // y ∈ [320, 330]
    for b in [right, left, top, bottom] {
        assert!(!vp.bbox_may_touch_screen(&b, 0), "off-screen bbox {b:?} should be rejected");
    }
}

#[test]
fn exact_margin_admits_one_pixel_beyond_rejects() {
    let vp = north_up();
    // Right edge: inclusive rect is x ∈ [0, 239], expanded to [-, 239 + M]. A point-bbox at
    // x = 239 + M lands exactly on the expanded edge (admit); at +1 it is beyond (reject).
    let m = INK_MARGIN;
    // x = lon + 120. Expanded right edge = 239 + m ⇒ lon = 119 + m.
    assert!(vp.bbox_may_touch_screen(&px(119 + m, 0), m), "exact right-margin pixel must be admitted");
    assert!(!vp.bbox_may_touch_screen(&px(120 + m, 0), m), "one pixel past the right margin must be rejected");

    // Left edge symmetry: expanded left edge = -m ⇒ x = -m ⇒ lon = -120 - m.
    assert!(vp.bbox_may_touch_screen(&px(-120 - m, 0), m), "exact left-margin pixel must be admitted");
    assert!(!vp.bbox_may_touch_screen(&px(-121 - m, 0), m), "one pixel past the left margin must be rejected");
}

#[test]
fn zero_margin_boundary_is_inclusive() {
    let vp = north_up();
    // With no margin the rightmost on-screen pixel is x = 239 ⇒ lon = 119; lon = 120 is off by one.
    assert!(vp.bbox_may_touch_screen(&px(119, 0), 0), "last on-screen pixel is inclusive");
    assert!(!vp.bbox_may_touch_screen(&px(120, 0), 0), "first off-screen pixel is rejected");
}

#[test]
fn max_width_cased_line_just_off_edge_still_admitted() {
    let vp = north_up();
    // A max-width cased road whose *centerline* bbox sits a few pixels past the right edge still
    // paints ink onto the panel: half of a 12-px stroke + casing reaches ~7 px inward. Its
    // centerline projects off-screen (rejected at margin 0) but the ink margin must admit it, or the
    // broad phase would clip a visible road — the exact false-negative this margin exists to prevent.
    let just_off = BBox { min_lon: 124, min_lat: -40, max_lon: 124, max_lat: 40 }; // x = 244, 5 px off
    assert!(!vp.bbox_may_touch_screen(&just_off, 0), "centerline alone is off-screen");
    assert!(vp.bbox_may_touch_screen(&just_off, INK_MARGIN), "ink reaches the panel — must not be rejected");

    // Same past a corner (both axes just off): still within the ink margin, still admitted.
    let corner = BBox { min_lon: 124, min_lat: 124, max_lon: 126, max_lat: 126 }; // x=244.., y=(-4)..
    assert!(vp.bbox_may_touch_screen(&corner, INK_MARGIN), "near-corner ink must not be rejected");
}

#[test]
fn rotated_view_rejects_enclosing_aabb_corners() {
    // The motivating case: a heading-up view rotated off the axes. Every corner of the map-space
    // `visible_bbox` lies outside the rotated screen rectangle, so a feature parked there passes the
    // map-space AABB test yet cannot paint a pixel. `bbox_may_touch_screen` must reject all four.
    // (Axis-aligned headings 0/90/180/270 of a square view have no empty corners — covered below.)
    for &course in &[FRAC_PI_4, 35.0_f32.to_radians(), 20.0_f32.to_radians()] {
        let vp = Viewport::new_rotated(240.0, 240.0, 0, 0, 1.0, course);
        let view = vp.visible_bbox();
        let corners = [
            point_bbox(view.min_lon, view.min_lat),
            point_bbox(view.min_lon, view.max_lat - 1),
            point_bbox(view.max_lon - 1, view.min_lat),
            point_bbox(view.max_lon - 1, view.max_lat - 1),
        ];
        for c in corners {
            // The map-space test alone would admit it (this is why the second test is needed)…
            assert!(c.intersects(&view), "corner probe must sit inside the enclosing AABB");
            // …but the screen test rejects the empty rotated corner.
            assert!(
                !vp.bbox_may_touch_screen(&c, 0),
                "course {course}: AABB corner {c:?} projects off the rotated screen"
            );
        }
        // And a bbox at the very centre is always admitted, whatever the heading.
        assert!(vp.bbox_may_touch_screen(&point_bbox(0, 0), 0), "centre is on-screen at any heading");
    }
}

#[test]
fn axis_aligned_headings_still_cull_correctly() {
    // Headings 0° and 90° of a square view leave the AABB equal to the screen (no empty corners),
    // but the helper must still admit on-screen features and reject off-screen ones under rotation.
    for &course in &[0.0, FRAC_PI_2, PI] {
        let vp = Viewport::new_rotated(240.0, 240.0, 0, 0, 1.0, course);
        assert!(vp.bbox_may_touch_screen(&point_bbox(0, 0), 0), "course {course}: centre admitted");
        assert!(!vp.bbox_may_touch_screen(&point_bbox(400, 400), 0), "course {course}: far corner rejected");
    }
}

#[test]
fn near_wraparound_heading_behaves_like_north_up() {
    // A course a hair under a full turn (~2π) is effectively north-up; the helper must not blow up
    // near the wrap and must still admit a centred bbox / reject a far one.
    let vp = Viewport::new_rotated(240.0, 240.0, 0, 0, 1.0, 2.0 * PI - 0.001);
    assert!(vp.bbox_may_touch_screen(&point_bbox(0, 0), 0));
    assert!(!vp.bbox_may_touch_screen(&point_bbox(500, 0), 0));
}

#[test]
fn extreme_wrapping_microdegree_coords_do_not_panic() {
    // Camera near the i32 antimeridian wrap; a feature "just across" it is a small delta via the
    // wrapping subtraction in `to_screen`, so it projects near centre and is admitted — and no
    // arithmetic overflows. Guards the extreme coords `Viewport` already supports.
    let vp = Viewport::new(240.0, 240.0, i32::MAX - 10, 0, 1.0);
    let across = BBox { min_lon: i32::MIN + 5, min_lat: -3, max_lon: i32::MIN + 15, max_lat: 3 };
    assert!(vp.bbox_may_touch_screen(&across, 0), "wrapped-delta feature projects on-screen");

    // A degenerate huge margin must saturate, not wrap into a bogus reject.
    assert!(vp.bbox_may_touch_screen(&point_bbox(0, 0), i32::MAX));
    // A far-off feature with a tiny margin is still rejected without panicking.
    let far = BBox { min_lon: i32::MAX - 5, min_lat: 1_000_000, max_lon: i32::MAX - 1, max_lat: 1_000_010 };
    let _ = vp.bbox_may_touch_screen(&far, 0); // only asserting no panic for this extreme
}

#[test]
fn route_chunk_near_edge_uses_the_same_broad_phase() {
    // Route chunks are culled by the same helper at the same 16-px margin. A chunk whose bbox is a
    // few pixels off-screen (its stroke/chevrons still reach the panel) is admitted; one well past
    // the margin is rejected. Mirrors the base-map contract for the route passes in `draw_route`.
    let vp = north_up();
    let near = BBox { min_lon: 123, min_lat: -50, max_lon: 123, max_lat: 50 }; // x = 243, 4 px off
    let far = BBox { min_lon: 200, min_lat: -50, max_lon: 200, max_lat: 50 }; // x = 320, well beyond
    assert!(vp.bbox_may_touch_screen(&near, INK_MARGIN), "near-edge route chunk admitted");
    assert!(!vp.bbox_may_touch_screen(&far, INK_MARGIN), "far route chunk rejected");
}
