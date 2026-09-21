#[path = "../src/present.rs"]
mod present;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obc_display::display_contracts::conformance::{self, GlassProbe};
use obc_display::ls021::{FRAME_H, FRAME_W};
use pollster::block_on;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_display::display_contracts::Device64Frame;
use obc_display::ls021::{row_hash, RowWindow};
use obc_display::{device64_to_rgb565, Band};
use obc_reader::rgb565_to_rgb888;
use present::*;

/// The sim's glass is the RGB888 texture, read back in the draw colour space. The expansion is
/// bit-replication, so truncation inverts it exactly.
impl<'b, const W: usize, const H: usize> GlassProbe<Device64Frame<'b, W, H>> for Present {
    fn glass(&self, x: u32, y: u32) -> Rgb565 {
        let t = (y as usize * self.width + x as usize) * 3;
        let (r, g, b) = (self.texture()[t], self.texture()[t + 1], self.texture()[t + 2]);
        Rgb565::new(r >> 3, g >> 2, b >> 3)
    }
}

/// Drive a present over a freshly-filled device-64 frame `v`.
fn present_fill(fb: &mut [u8], p: &mut Present, v: u8, exclude: Option<(u16, u16)>) {
    fb.fill(v);
    p.present_now(fb, exclude);
}

/// The RGB888 the texture should hold for a device-64 byte.
fn expect_px(byte: u8) -> (u8, u8, u8) {
    rgb565_to_rgb888(device64_to_rgb565(byte))
}

fn rgb(raw: u16) -> Rgb565 {
    Rgb565::from(RawU16::new(raw))
}

// The generic conformance suite, the same checks the board-semantics double runs, against this
// backend: the simulator presenter paired with `Device64Frame`, through the real contract impls.

const CW: usize = 16;
const CH: usize = 16;
/// The reference overlay window: a right-edge rect, which is the bulge shape scaled down.
const OVERLAY: Rectangle = Rectangle { top_left: Point::new(12, 4), size: Size::new(4, 4) };
const RED: u16 = 0xF800;
const GREEN: u16 = 0x07E0;
const BLUE: u16 = 0x001F;

#[test]
fn conformance_full_present() {
    let mut buf = [0u8; CW * CH];
    let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
    let mut p = Present::new(CW as u32, CH as u32);
    block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn conformance_damage_translation() {
    let mut buf = [0u8; CW * CH];
    let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
    let mut p = Present::new(CW as u32, CH as u32);
    block_on(conformance::check_damage_translation(&mut frame, &mut p, rgb(RED), rgb(BLUE), true));
}

#[test]
fn conformance_overlay_backdrop() {
    let mut buf = [0u8; CW * CH];
    let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
    let mut p = Present::new(CW as u32, CH as u32);
    block_on(conformance::check_overlay_backdrop(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        OVERLAY,
        (13, 5),
        (14, 6),
        |f| f.bytes().to_vec(),
    ));
}

#[test]
fn conformance_overlay_exclusion() {
    let mut buf = [0u8; CW * CH];
    let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
    let mut p = Present::new(CW as u32, CH as u32);
    block_on(conformance::check_overlay_exclusion(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        rgb(GREEN),
        OVERLAY,
        (13, 5),
        (14, 6),
        (0, 0),
        |f| f.bytes().to_vec(),
        true,
    ));
}

#[test]
fn conformance_overlay_pop_retract_clear() {
    let mut buf = [0u8; CW * CH];
    let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
    let mut p = Present::new(CW as u32, CH as u32);
    block_on(conformance::check_overlay_pop_retract_clear(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        OVERLAY,
        (12, 5),
        (15, 6),
        |f| f.bytes().to_vec(),
        true,
    ));
}

/// The device-geometry contract pairing: the conformance exclusion check at the shipping
/// resolution with the real bulge window shape, so the trait path is proven at the geometry the GUI
/// runs.
#[test]
fn conformance_overlay_exclusion_at_device_geometry() {
    let mut buf = vec![0u8; FRAME_W * FRAME_H];
    let mut frame = Device64Frame::<FRAME_W, FRAME_H>::new(&mut buf);
    let mut p = Present::new(FRAME_W as u32, FRAME_H as u32);
    // The real bulge shape: a right-edge 16-column window.
    let overlay = Rectangle::new(Point::new((FRAME_W - 16) as i32, 60), Size::new(16, 111));
    block_on(conformance::check_overlay_exclusion(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        rgb(GREEN),
        overlay,
        (FRAME_W as u32 - 8, 100),
        (FRAME_W as u32 - 2, 140),
        (0, 0),
        |f| f.bytes().to_vec(),
        true,
    ));
}

#[test]
fn geometry_matches_the_platform_authority() {
    // The sim's device resolution comes from the one display authority.
    assert_eq!(FRAME_W, 240);
    assert_eq!(FRAME_H, 320);
    let p = Present::new(FRAME_W as u32, FRAME_H as u32);
    assert_eq!(p.width, FRAME_W);
    assert_eq!(p.rows, FRAME_H);
}

#[test]
fn first_present_pushes_the_whole_frame() {
    let mut fb = vec![0u8; 2 * 4];
    let mut p = Present::new(2, 4);
    present_fill(&mut fb, &mut p, 0x15, None);
    assert_eq!(p.stats.pushed_rows, 4, "the first present pushes the whole frame");
    assert_eq!(p.stats.spans, 1);
    // The texture reconstructs every pixel from the device-64 byte.
    let (r, g, b) = expect_px(0x15);
    assert!(p.texture().as_chunks::<3>().0.iter().all(|px| *px == [r, g, b]), "texture reconstructs the whole frame");
}

#[test]
fn identical_reframe_pushes_nothing() {
    let mut fb = vec![0u8; 2 * 4];
    let mut p = Present::new(2, 4);
    present_fill(&mut fb, &mut p, 0x15, None);
    present_fill(&mut fb, &mut p, 0x15, None);
    assert_eq!(p.stats.pushed_rows, 0, "a byte-identical reframe pushes nothing");
    assert_eq!(p.stats.spans, 0);
}

#[test]
fn only_the_changed_band_is_pushed_and_texture_reconstructs_it() {
    let mut fb = vec![0u8; 2 * 5];
    let mut p = Present::new(2, 5);
    present_fill(&mut fb, &mut p, 0x00, None);
    // Change only row 2.
    fb[2 * 2..3 * 2].fill(0x2A);
    p.present_now(&fb, None);
    assert_eq!(p.stats.pushed_rows, 1, "only the one changed row is pushed");
    assert_eq!(p.stats.spans, 1);
    // The partial push reconstructs row 2 in the texture, and the others stay black.
    let (r, g, b) = expect_px(0x2A);
    let px = |row: usize, col: usize| {
        let t = (row * 2 + col) * 3;
        (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
    };
    assert_eq!(px(2, 0), (r, g, b));
    assert_eq!(px(2, 1), (r, g, b));
    assert_eq!(px(1, 0), (0, 0, 0), "unchanged rows stay black");
}

#[test]
fn present_honours_the_overlay_exclude_span() {
    // Rows 1 to 3 change, and the excluded rows belong to the overlay plane, so the clean present
    // pushes only the first of them.
    let mut fb = vec![0u8; 2 * 5];
    let mut p = Present::new(2, 5);
    present_fill(&mut fb, &mut p, 0x00, None);
    for y in 1..=3 {
        fb[y * 2..(y + 1) * 2].fill(0x11);
    }
    p.present_now(&fb, Some((2, 2)));
    assert_eq!(p.stats.pushed_rows, 1, "the excluded rows are not pushed by the clean present");
    assert_eq!(p.stats.spans, 1);
    // The oracle proved the pushed span and the exclude span cover every real change. The
    // excluded rows must also have stayed black in the texture, because the overlay owns them.
    let row1 = expect_px(0x11);
    let at = |row: usize| {
        let t = row * 2 * 3;
        (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
    };
    assert_eq!(at(1), row1, "row 1 pushed clean");
    assert_eq!(at(2), (0, 0, 0), "excluded row left for the overlay plane");
}

#[test]
fn present_overlay_composites_the_backdrop_then_the_drawer_over_it() {
    // A device-64 backdrop with a distinct byte per pixel, then an overlay window that paints one
    // pixel red over the clean frame, through the same shared helper the device uses.
    let mut fb = vec![0u8; 8 * 8];
    let mut p = Present::new(8, 8);
    for (i, b) in fb.iter_mut().enumerate() {
        *b = (i as u8) & 0b0011_1111;
    }
    // Seed the texture from a full present, so unrelated pixels are defined.
    p.present_now(&fb, None);
    let fb_snapshot = fb.clone();
    // The drawer paints one frame-absolute pixel red inside the window.
    let region = RowWindow { x0: 4, y0: 2, w: 4, rows: 4 };
    p.present_overlay_now(&fb, region, &mut |band: &mut Band| {
        band.fill_solid(&Rectangle::new(Point::new(5, 3), Size::new(1, 1)), Rgb565::from(RawU16::new(0xF800))).ok();
    });
    let tex_at = |x: usize, y: usize| {
        let t = (y * 8 + x) * 3;
        (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
    };
    // The backdrop pixel is its frame byte expanded to RGB888.
    assert_eq!(tex_at(4, 2), expect_px(20), "backdrop = clean fb expanded to RGB888");
    // The overlay pixel is pure red.
    assert_eq!(tex_at(5, 3), (255, 0, 0), "the drawer painted frame-absolute (5,3) red");
    // The overlay path never writes the clean framebuffer.
    assert_eq!(fb, fb_snapshot, "present_overlay never writes the resident frame");
}

/// One failed oracle check must not desync later presents. This fabricates the aftermath of a
/// row-hash collision, a changed row whose store hash already matches, beside an honestly changed
/// row: the assert fires, but the honest row was pushed before it, and the stale row self-heals on
/// its next change.
#[test]
fn a_missed_row_cannot_poison_subsequent_presents() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let mut fb = vec![0u8; 4 * 6];
    let mut p = Present::new(4, 6);
    present_fill(&mut fb, &mut p, 0x01, None);
    // Row 2 changes but its store hash is already up to date: a simulated collision.
    fb[2 * 4..3 * 4].fill(0x22);
    p.hashes[2] = row_hash(&fb[2 * 4..3 * 4]);
    // Row 4 changes honestly in the same frame.
    fb[4 * 4..5 * 4].fill(0x2A);
    let outcome = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
    if cfg!(debug_assertions) {
        assert!(outcome.is_err(), "the oracle assert fires on the fabricated miss");
    }
    // The frame landed before the assert aborted the present.
    assert_eq!(&p.presented[4 * 4..5 * 4], &[0x2A; 4], "the honest row was pushed despite the failed oracle");
    // The stale row heals the moment it changes again, and nothing else re-asserts.
    fb[2 * 4..3 * 4].fill(0x30);
    p.present_now(&fb, None);
    assert_eq!(p.presented, fb, "one missed row healed itself; no cascade");
}

/// The oracle's miss diagnostics are deduped per row: a missed row on a parked screen stays
/// byte-different every frame, so the report must fire once when the miss appears, stay quiet while
/// it persists, and re-arm when the row heals.
#[test]
fn miss_diagnostics_are_deduped_per_row() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let mut fb = vec![0u8; 4 * 6];
    let mut p = Present::new(4, 6);
    present_fill(&mut fb, &mut p, 0x01, None);
    // Fabricate a persistent miss on row 2, as a collision would leave.
    fb[2 * 4..3 * 4].fill(0x22);
    p.hashes[2] = row_hash(&fb[2 * 4..3 * 4]);
    // On the first present the miss appears and is reported. The debug assert also fires.
    let _ = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
    assert!(p.miss_reported[2], "the new miss is reported");
    assert_eq!(p.misses_flagged, 1);
    // The screen stays parked and the same miss persists, so there is no re-report.
    for _ in 0..3 {
        let _ = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
        assert!(p.miss_reported[2], "the persisting miss stays flagged, not re-reported");
        assert_eq!(p.misses_flagged, 1, "no duplicate report accumulates");
    }
    // The row changes, is re-pushed and heals, so the report re-arms.
    fb[2 * 4..3 * 4].fill(0x30);
    p.present_now(&fb, None);
    assert!(!p.miss_reported[2], "a healed row re-arms its report");
    assert_eq!(p.misses_flagged, 0);
}

/// Two frames that differ only in the device-64 pixels at columns 3 and 7, both in the top byte of
/// a hash word. A word-wise FNV row hash collides on this pair, so the diff would skip the row. The
/// row hash must flag and push it.
#[test]
fn lane3_confined_pixel_change_is_pushed() {
    let mut fb = vec![0u8; 8 * 4];
    let mut p = Present::new(8, 4);
    present_fill(&mut fb, &mut p, 0x00, None);
    // Row 2 carries a pair of pixel values that collide under a word-wise hash.
    fb[2 * 8 + 3] = 0x02;
    fb[2 * 8 + 7] = 0x2E;
    p.present_now(&fb, None);
    assert_eq!(p.stats.pushed_rows, 1, "the lane-3-confined change must be diffed and pushed");
    let r = 2 * 8..3 * 8;
    assert_eq!(p.presented[r.clone()], fb[r], "row 2 reconstructed");
}

#[test]
fn texture_tracks_a_sequence_of_partial_changes() {
    let mut fb = vec![0u8; 3 * 6];
    let mut p = Present::new(3, 6);
    present_fill(&mut fb, &mut p, 0x04, None);
    // A few disjoint edits across frames. After each, only the changed row is pushed and the
    // texture reconstructs it, which is what makes partial pushes reconstruct the whole.
    for (row, val) in [(0usize, 0x08u8), (5, 0x0C), (3, 0x10)] {
        fb[row * 3..(row + 1) * 3].fill(val);
        p.present_now(&fb, None);
        assert_eq!(p.stats.pushed_rows, 1, "only row {row} pushed");
        let (r, g, b) = expect_px(val);
        let t = row * 3 * 3;
        assert_eq!((p.texture()[t], p.texture()[t + 1], p.texture()[t + 2]), (r, g, b), "row {row} in texture");
    }
}
