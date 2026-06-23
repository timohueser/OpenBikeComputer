//! Projection / viewport coverage for the renderer (issue #96, epic #90).
//!
//! The projection is the numeric heart of the renderer, yet every other render test draws
//! north-up and probes pixels, never the `Viewport` transform itself. This file pins it directly:
//! the `to_map`∘`to_screen` involution to sub-pixel tolerance, the rotated (heading-up) projection,
//! `visible_bbox` over a rotated view, and `north_screen_unit` / `aspect_for_lat` — the helpers
//! everything else builds on (lib.rs ~187-263).

use obc_render::Viewport;

/// Ground microdegrees per screen pixel along latitude at `zoom` (pixels per microdegree-lat).
fn microdeg_per_px(zoom: f32) -> f32 {
    1.0 / zoom
}

// ---------------------------------------------------------------------------
// Render item 1 — projection involution, rotation, visible_bbox, north unit
// ---------------------------------------------------------------------------

/// `to_map(to_screen(p)) ≈ p` — the projection is its own inverse (lib.rs ~208). `to_screen` rounds
/// to integer pixels, so the round-trip can't be bit-exact; the only error is that ½-px
/// quantization, which in ground units is `0.5 / zoom` microdegrees of latitude (and the
/// aspect-scaled equivalent in longitude). We assert the round-trip stays within ~1 px of ground —
/// far tighter than the microdegree grid would allow if a sign or an aspect factor were wrong.
#[test]
fn to_map_inverts_to_screen_within_subpixel() {
    let vp = Viewport::new(240.0, 320.0, 11_000_000, 47_000_000, 4.0); // ~Innsbruck, 4 px/µdeg-lat
    let tol_lat = microdeg_per_px(vp.zoom); // 1 px of latitude in µdeg
    let tol_lon = tol_lat / vp.aspect; // longitude is aspect-compressed, so 1 px is more µdeg

    // Sample points around the camera (in µdeg offsets), all well within the view.
    for &(dlon, dlat) in &[(0, 0), (30, 0), (0, 25), (-40, 15), (50, -35), (-20, -45)] {
        let (lon, lat) = (vp.cam_lon + dlon, vp.cam_lat + dlat);
        let (sx, sy) = vp.to_screen(lon, lat);
        let (rlon, rlat) = vp.to_map(sx as f32, sy as f32);
        assert!(
            (rlon - lon).abs() as f32 <= tol_lon + 1.0,
            "lon round-trip off by {} µdeg (tol {tol_lon})",
            (rlon - lon).abs()
        );
        assert!(
            (rlat - lat).abs() as f32 <= tol_lat + 1.0,
            "lat round-trip off by {} µdeg (tol {tol_lat})",
            (rlat - lat).abs()
        );
    }
}

/// At north-up (course 0) a point due north of the camera (higher latitude, same longitude)
/// projects straight up the screen — same x as the camera center, smaller y — and a point due east
/// projects to the right. This is the baseline the rotated cases are measured against (lib.rs
/// ~187).
#[test]
fn north_up_projects_north_to_screen_up() {
    let vp = Viewport::new(200.0, 200.0, 0, 0, 1.0);
    let (cx, cy) = vp.to_screen(0, 0); // camera center → screen center
    assert_eq!((cx, cy), (100, 100));

    let (nx, ny) = vp.to_screen(0, 1000); // due north
    assert_eq!(nx, cx, "due-north keeps the camera's screen x at north-up");
    assert!(ny < cy, "north is up the screen (smaller y)");

    let (ex, ey) = vp.to_screen(1000, 0); // due east
    assert!(ex > cx, "east is to the right");
    assert_eq!(ey, cy, "due-east keeps the camera's screen y at north-up");
}

/// Heading-up: with `course_rad = 90°` (camera facing east) the projection rotates so the heading
/// points up the screen. A point due *east* of the camera (the direction of travel) must therefore
/// project toward the **top**, and map-north must swing to the **left**. This exercises the
/// `sin_c`/`cos_c` rotation in `to_screen` (lib.rs ~195) that the north-up suite never turns on.
#[test]
fn heading_up_rotates_travel_direction_to_screen_top() {
    use core::f32::consts::FRAC_PI_2;
    let vp = Viewport::new_rotated(200.0, 200.0, 0, 0, 1.0, FRAC_PI_2); // facing east
    let (cx, cy) = vp.to_screen(0, 0);
    assert_eq!((cx, cy), (100, 100));

    // Due east = the heading → up the screen.
    let (ex, ey) = vp.to_screen(1000, 0);
    assert!(ey < cy, "the travel direction (east) points up the screen (got y={ey})");
    assert!((ex - cx).abs() <= 1, "the heading direction has ~no cross-screen component (x={ex})");

    // Map-north swings 90° to screen-left.
    let (nx, ny) = vp.to_screen(0, 1000);
    assert!(nx < cx, "map-north rotates to the left (got x={nx})");
    assert!((ny - cy).abs() <= 1, "north has ~no up/down component when heading east (y={ny})");
}

/// `north_screen_unit` is the unit screen vector pointing to map-north — the compass needle (lib.rs
/// ~255). At north-up it is straight up `(0, -1)`; under a heading-up rotation it turns by the same
/// course. We check both, and that it stays unit length (the doc claims it needs no normalization),
/// and that it agrees in *direction* with where `to_screen` actually puts a due-north point.
#[test]
fn north_screen_unit_tracks_the_rotation() {
    // North-up: straight up.
    let vp0 = Viewport::new(200.0, 200.0, 0, 0, 1.0);
    let (ux, uy) = vp0.north_screen_unit();
    assert!((ux - 0.0).abs() < 1e-6 && (uy + 1.0).abs() < 1e-6, "north-up needle points straight up, got ({ux},{uy})");

    // Heading east (90°): north points screen-left → unit vector ≈ (-1, 0).
    use core::f32::consts::FRAC_PI_2;
    let vp1 = Viewport::new_rotated(200.0, 200.0, 0, 0, 1.0, FRAC_PI_2);
    let (vx, vy) = vp1.north_screen_unit();
    assert!((vx + 1.0).abs() < 1e-5 && vy.abs() < 1e-5, "heading-east needle points left, got ({vx},{vy})");

    // Unit length at an arbitrary course.
    let vp2 = Viewport::new_rotated(200.0, 200.0, 0, 0, 1.0, 0.7);
    let (wx, wy) = vp2.north_screen_unit();
    assert!(((wx * wx + wy * wy).sqrt() - 1.0).abs() < 1e-5, "the needle is unit length");
}

/// `visible_bbox` over a **rotated** view must cover the tilted on-screen rectangle's full extent —
/// it takes all four screen corners, so the axis-aligned ground box grows wider than the north-up
/// box at the same zoom (lib.rs ~225). A 45° course is the worst case: the diagonal of the screen
/// becomes the bbox's half-extent. We assert the rotated box strictly contains the north-up box and
/// that the camera center is inside it.
#[test]
fn rotated_visible_bbox_covers_the_tilted_rectangle() {
    let up = Viewport::new(200.0, 200.0, 0, 0, 1.0);
    let bb_up = up.visible_bbox();

    use core::f32::consts::FRAC_PI_4;
    let rot = Viewport::new_rotated(200.0, 200.0, 0, 0, 1.0, FRAC_PI_4); // 45°
    let bb_rot = rot.visible_bbox();

    // The camera center is inside both.
    for bb in [&bb_up, &bb_rot] {
        assert!(bb.min_lon <= 0 && bb.max_lon >= 0 && bb.min_lat <= 0 && bb.max_lat >= 0, "center inside the bbox");
    }
    // A 45° tilt of a square view widens the axis-aligned cover in both axes (the corner that was
    // on the edge now reaches a screen corner). So the rotated box strictly contains the up box.
    assert!(bb_rot.min_lon < bb_up.min_lon, "rotated bbox extends further west");
    assert!(bb_rot.max_lon > bb_up.max_lon, "rotated bbox extends further east");
    assert!(bb_rot.min_lat < bb_up.min_lat, "rotated bbox extends further south");
    assert!(bb_rot.max_lat > bb_up.max_lat, "rotated bbox extends further north");
}

/// Aspect correction compresses longitude away from the equator: at higher latitude a degree of
/// longitude spans less ground, so the same µdeg-lon step projects to fewer pixels. The renderer
/// folds this into `Viewport::aspect` (= cos(lat)); a viewport built at a high latitude must carry
/// a smaller aspect than one at the equator, and an equatorial viewport's aspect is ~1. This pins
/// `aspect_for_lat` (lib.rs ~261), exercised only indirectly elsewhere.
#[test]
fn aspect_compresses_longitude_with_latitude() {
    let equator = Viewport::new(200.0, 200.0, 0, 0, 1.0);
    let high = Viewport::new(200.0, 200.0, 0, 60_000_000, 1.0); // 60°N

    assert!((equator.aspect - 1.0).abs() < 1e-3, "aspect ≈ 1 at the equator, got {}", equator.aspect);
    // cos(60°) = 0.5.
    assert!((high.aspect - 0.5).abs() < 1e-3, "aspect ≈ cos(60°) = 0.5 at 60°N, got {}", high.aspect);
    assert!(high.aspect < equator.aspect, "longitude is more compressed further from the equator");

    // The compression shows up in projection: a fixed µdeg-lon step is fewer pixels at 60°N.
    let dx_eq = equator.to_screen(1000, 0).0 - equator.to_screen(0, 0).0;
    let dx_hi = high.to_screen(1000, 60_000_000).0 - high.to_screen(0, 60_000_000).0;
    assert!(dx_hi < dx_eq, "the same lon step spans fewer px at 60°N ({dx_hi} < {dx_eq})");
}
