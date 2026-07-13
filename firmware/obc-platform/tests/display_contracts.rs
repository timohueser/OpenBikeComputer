//! FAR-12 (#805) contract tests: the conformance suite run against **two pairings** with nothing in
//! common but the contracts —
//!
//! 1. the **LS021-semantics reference pairing** — `Device64Frame` (RGB222, one byte per pixel) +
//!    [`SpanPresenter`], the migration template for #806's real backends: the row-hash self-diff
//!    ([`RowDiff`]), span masking, full-width-row overlay grain, the shared
//!    [`composite_overlay_window`] composite, and the exact-diff oracle
//!    ([`spans_missed_changes`]) asserted inside every present;
//! 2. the **compile-only proof pairing** — [`MiniFrame`] (16×8, *padded* 20-cell stride, native
//!    RGB565 `u16` cells) + [`TilePresenter`] (4×4-tile damage grain, bounded-scratch overlay
//!    composite). It exists to prove the contracts hard-code no LS021 assumption; it is test-only
//!    code, never linked into a shipping image, and no RGB222 byte ever passes through its format.
//!
//! Plus the [`BridgedDriver`] check: the legacy `DisplayDriver` seam driven over pairing 1.

use core::convert::Infallible;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_platform::display_contracts::conformance::{self, GlassProbe};
use obc_platform::display_contracts::{
    BridgedDriver, Device64Frame, NativeFrame, OverlayPresenter, PresentStats, Presenter,
};
use obc_platform::{
    composite_overlay_window, device64_to_rgb565, spans_missed_changes, Band, DisplayDriver, FbDevice64, OverlayRegion,
    RowDiff,
};
use obc_reader::rgb565_to_device64;
use pollster::block_on;

fn rgb(raw: u16) -> Rgb565 {
    Rgb565::from(RawU16::new(raw))
}
const RED: u16 = 0xF800;
const GREEN: u16 = 0x07E0;
const BLUE: u16 = 0x001F;

/// A simulated transport fault (a stalled FLPR / SPI error) — the outcome a caller retries on.
#[derive(Debug)]
struct Fault;

// ─── Pairing 1: the LS021-semantics reference presenter over `Device64Frame` ───

/// The LS021 damage strategy behind the contracts, minus the FLPR transport: per-row-hash
/// self-diff, exclusion clipped by the shared [`RowDiff::diff_clipped`], the shared window
/// composite, and a host "glass" reconstruction updated only from pushed spans — checked by the
/// exact-diff oracle on every present, exactly like the simulator backend.
struct SpanPresenter<const W: usize, const H: usize> {
    diff: RowDiff<H>,
    /// Device-64 bytes on glass, reconstructed from partial pushes (the oracle's `prev`).
    glass: Vec<u8>,
    /// Inject one transport fault into the next present.
    fail_next: bool,
}

impl<const W: usize, const H: usize> SpanPresenter<W, H> {
    fn new() -> Self {
        Self { diff: RowDiff::new(), glass: vec![0; W * H], fail_next: false }
    }
}

enum SpanDamage {
    /// Forced full repaint: re-seed the row-hash store and push every row.
    Full,
    /// Self-diff, optionally clipping a live overlay's rows out (the seam's `exclude`).
    SelfDiff { exclude: Option<(u16, u16)> },
}

/// The LS021 overlay grain: a column window on full-width rows (the panel re-latches whole rows).
#[derive(Clone, Copy)]
struct RowWindow {
    x0: u16,
    y0: u16,
    w: u16,
    rows: u16,
}

impl<'b, const W: usize, const H: usize> Presenter<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    type Damage = SpanDamage;
    type Error = Fault;

    fn damage_full() -> SpanDamage {
        SpanDamage::Full
    }

    fn damage_unknown() -> SpanDamage {
        SpanDamage::SelfDiff { exclude: None }
    }

    async fn present(&mut self, frame: &Device64Frame<'b, W, H>, damage: SpanDamage) -> Result<PresentStats, Fault> {
        if std::mem::take(&mut self.fail_next) {
            return Err(Fault);
        }
        let exclude = match damage {
            SpanDamage::Full => {
                self.diff.reset(); // full = re-seed the store + push everything, like the board's recovery path
                None
            }
            SpanDamage::SelfDiff { exclude } => exclude,
        };
        let mut scratch = [(0u16, 0u16); 16];
        let spans = self.diff.diff_clipped(frame.bytes(), W, exclude, &mut scratch);
        let regions = spans.len() as u32;
        let mut pushed = 0u32;
        for &(y0, n) in spans {
            let r = y0 as usize * W..(y0 + n) as usize * W;
            self.glass[r.clone()].copy_from_slice(&frame.bytes()[r]);
            pushed += n as u32;
        }
        // The exact-diff oracle: pushed spans + the overlay-owned excluded rows cover every change.
        let mut oracle = spans.to_vec();
        if let Some(ex) = exclude {
            oracle.push(ex);
        }
        let mut covered = vec![false; H];
        assert_eq!(spans_missed_changes(&self.glass, frame.bytes(), W, H, &oracle, &mut covered), 0);
        Ok(PresentStats { pushed_units: pushed, total_units: H as u32, regions })
    }
}

impl<'b, const W: usize, const H: usize> OverlayPresenter<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    type Region = RowWindow;
    type OverlayTarget<'t> = Band<'t>;

    fn region(rect: Rectangle) -> RowWindow {
        let c = rect.intersection(&Rectangle::new(Point::zero(), Size::new(W as u32, H as u32)));
        RowWindow {
            x0: c.top_left.x as u16,
            y0: c.top_left.y as u16,
            w: c.size.width as u16,
            rows: c.size.height as u16,
        }
    }

    fn damage_around(r: RowWindow) -> SpanDamage {
        SpanDamage::SelfDiff { exclude: Some((r.y0, r.rows)) }
    }

    async fn present_overlay(
        &mut self,
        frame: &mut Device64Frame<'b, W, H>,
        r: RowWindow,
        draw: impl for<'t> FnOnce(&mut Band<'t>),
    ) -> Result<PresentStats, Fault> {
        if std::mem::take(&mut self.fail_next) {
            return Err(Fault);
        }
        // Composite over the clean frame through the one shared helper the device + sim use.
        let (w, rows) = (r.w as usize, r.rows as usize);
        let mut scratch = vec![0u16; w * rows];
        let window = Rectangle::new(Point::new(r.x0 as i32, r.y0 as i32), Size::new(r.w as u32, r.rows as u32));
        let mut draw = Some(draw);
        composite_overlay_window(frame.bytes(), Size::new(W as u32, H as u32), window, &mut scratch, &mut |band| {
            if let Some(d) = draw.take() {
                d(band)
            }
        });
        // LS021 grain: re-latch the full-width rows [y0, y0+rows) — clean frame bytes everywhere,
        // the composited window re-quantized over its columns (the FLPR backend does the same
        // transiently in the resident frame; the frame stays untouched here, glass carries it).
        for row in 0..rows {
            let y = r.y0 as usize + row;
            self.glass[y * W..(y + 1) * W].copy_from_slice(&frame.bytes()[y * W..(y + 1) * W]);
            for col in 0..w {
                let (dr, dg, db) = rgb565_to_device64(scratch[row * w + col]);
                self.glass[y * W + r.x0 as usize + col] = ((dr / 85) << 4) | ((dg / 85) << 2) | (db / 85);
            }
        }
        Ok(PresentStats { pushed_units: r.rows as u32, total_units: H as u32, regions: 1 })
    }
}

impl<'b, const W: usize, const H: usize> GlassProbe<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    fn glass(&self, x: u32, y: u32) -> Rgb565 {
        rgb(device64_to_rgb565(self.glass[y as usize * W + x as usize]))
    }
}

// ─── Pairing 2: the compile-only proof backend (different geometry / storage / grain) ───

const MW: usize = 16;
const MH: usize = 8;
const MSTRIDE: usize = 20; // padded backing: 4 unused cells per row
const TILE: usize = 4;
const TILES_X: usize = MW / TILE;
const TILES: usize = TILES_X * (MH / TILE); // 8 tiles

/// 16×8 frame of native-RGB565 `u16` cells on a padded 20-cell stride — nothing like the shipping
/// RGB222 plane, which is the point.
struct MiniFrame {
    cells: [u16; MSTRIDE * MH],
}

/// The direct-write draw view over the padded backing (frame-absolute, clip-checked stores).
struct MiniTarget<'a> {
    cells: &'a mut [u16],
    clip: Rectangle,
}

impl OriginDimensions for MiniTarget<'_> {
    fn size(&self) -> Size {
        Size::new(MW as u32, MH as u32)
    }
}

impl DrawTarget for MiniTarget<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Infallible> {
        for Pixel(p, c) in pixels {
            if self.clip.contains(p) {
                self.cells[p.y as usize * MSTRIDE + p.x as usize] = c.into_storage();
            }
        }
        Ok(())
    }
}

fn mini_bounds() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(MW as u32, MH as u32))
}

impl NativeFrame for MiniFrame {
    type Color = Rgb565;
    type Pixel = u16;
    const WIDTH: usize = MW;
    const HEIGHT: usize = MH;
    const STRIDE: usize = MSTRIDE;

    fn backing(&self) -> &[u16] {
        &self.cells
    }

    fn backing_mut(&mut self) -> &mut [u16] {
        &mut self.cells
    }

    fn draw_target(&mut self) -> impl DrawTarget<Color = Rgb565, Error = Infallible> + '_ {
        MiniTarget { cells: &mut self.cells, clip: mini_bounds() }
    }

    fn clipped(&mut self, area: Rectangle) -> impl DrawTarget<Color = Rgb565, Error = Infallible> + '_ {
        MiniTarget { cells: &mut self.cells, clip: area.intersection(&mini_bounds()) }
    }
}

/// A tile bitmask — the proof backend's region *and* exclusion grain.
#[derive(Clone, Copy)]
struct TileSet(u8);

enum TileDamage {
    Full,
    Diff { exclude: TileSet },
}

fn tile_rect(t: usize) -> Rectangle {
    Rectangle::new(
        Point::new(((t % TILES_X) * TILE) as i32, ((t / TILES_X) * TILE) as i32),
        Size::new(TILE as u32, TILE as u32),
    )
}

/// Tile-grained presenter: damage = exact per-tile compare against its glass copy, overlay =
/// bounded-scratch composite. Its glass is *tightly* packed (stride `MW`, not the frame's padded
/// `MSTRIDE`), so any stride-blind indexing in the contracts would shear visibly here.
struct TilePresenter {
    glass: [u16; MW * MH],
}

impl TilePresenter {
    fn new() -> Self {
        Self { glass: [0; MW * MH] }
    }

    fn tile_differs(&self, frame: &MiniFrame, t: usize) -> bool {
        let tl = tile_rect(t).top_left;
        (0..TILE).any(|dy| {
            let (y, x0) = (tl.y as usize + dy, tl.x as usize);
            frame.cells[y * MSTRIDE + x0..y * MSTRIDE + x0 + TILE] != self.glass[y * MW + x0..y * MW + x0 + TILE]
        })
    }

    fn push_tile(&mut self, src: &[u16], src_stride: usize, t: usize) {
        let tl = tile_rect(t).top_left;
        for dy in 0..TILE {
            let (y, x0) = (tl.y as usize + dy, tl.x as usize);
            self.glass[y * MW + x0..y * MW + x0 + TILE]
                .copy_from_slice(&src[y * src_stride + x0..y * src_stride + x0 + TILE]);
        }
    }
}

impl Presenter<MiniFrame> for TilePresenter {
    type Damage = TileDamage;
    type Error = Fault;

    fn damage_full() -> TileDamage {
        TileDamage::Full
    }

    fn damage_unknown() -> TileDamage {
        TileDamage::Diff { exclude: TileSet(0) }
    }

    async fn present(&mut self, frame: &MiniFrame, damage: TileDamage) -> Result<PresentStats, Fault> {
        let (full, exclude) = match damage {
            TileDamage::Full => (true, TileSet(0)),
            TileDamage::Diff { exclude } => (false, exclude),
        };
        let mut pushed = 0u32;
        for t in 0..TILES {
            if exclude.0 & (1 << t) == 0 && (full || self.tile_differs(frame, t)) {
                self.push_tile(&frame.cells, MSTRIDE, t);
                pushed += 1;
            }
        }
        Ok(PresentStats { pushed_units: pushed, total_units: TILES as u32, regions: pushed })
    }
}

/// Frame-absolute draw view over the overlay composite scratch, clipped to the region's tiles.
struct MiniOverlay<'a> {
    cells: &'a mut [u16],
    region: TileSet,
}

impl OriginDimensions for MiniOverlay<'_> {
    fn size(&self) -> Size {
        Size::new(MW as u32, MH as u32)
    }
}

impl DrawTarget for MiniOverlay<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Infallible> {
        for Pixel(p, c) in pixels {
            if mini_bounds().contains(p) {
                let t = (p.y as usize / TILE) * TILES_X + p.x as usize / TILE;
                if self.region.0 & (1 << t) != 0 {
                    self.cells[p.y as usize * MW + p.x as usize] = c.into_storage();
                }
            }
        }
        Ok(())
    }
}

impl OverlayPresenter<MiniFrame> for TilePresenter {
    type Region = TileSet;
    type OverlayTarget<'t> = MiniOverlay<'t>;

    fn region(rect: Rectangle) -> TileSet {
        let mut mask = 0u8;
        for t in 0..TILES {
            if !tile_rect(t).intersection(&rect).is_zero_sized() {
                mask |= 1 << t;
            }
        }
        TileSet(mask)
    }

    fn damage_around(region: TileSet) -> TileDamage {
        TileDamage::Diff { exclude: region }
    }

    async fn present_overlay(
        &mut self,
        frame: &mut MiniFrame,
        region: TileSet,
        draw: impl for<'t> FnOnce(&mut MiniOverlay<'t>),
    ) -> Result<PresentStats, Fault> {
        // Bounded scratch composite (a different strategy from the FLPR's composite-into-frame):
        // backdrop = the clean frame's region tiles, then the drawer, then push scratch → glass.
        // The frame is provably untouched — this impl never writes it.
        let mut scratch = [0u16; MW * MH];
        let mut pushed = 0u32;
        for t in (0..TILES).filter(|t| region.0 & (1 << t) != 0) {
            let tl = tile_rect(t).top_left;
            for dy in 0..TILE {
                let (y, x0) = (tl.y as usize + dy, tl.x as usize);
                scratch[y * MW + x0..y * MW + x0 + TILE]
                    .copy_from_slice(&frame.cells[y * MSTRIDE + x0..y * MSTRIDE + x0 + TILE]);
            }
        }
        draw(&mut MiniOverlay { cells: &mut scratch, region });
        for t in (0..TILES).filter(|t| region.0 & (1 << t) != 0) {
            self.push_tile(&scratch, MW, t);
            pushed += 1;
        }
        Ok(PresentStats { pushed_units: pushed, total_units: TILES as u32, regions: pushed })
    }
}

impl GlassProbe<MiniFrame> for TilePresenter {
    fn glass(&self, x: u32, y: u32) -> Rgb565 {
        rgb(self.glass[y as usize * MW + x as usize])
    }
}

// ─── Conformance runs ───

// The reference pairing's probe geometry: a right-edge 4×4 overlay window on a 16×16 frame (the
// bulge shape). The LS021 widening is full-width rows, so "outside" = a row clear of [4, 8).
const SPAN_RECT: Rectangle = Rectangle { top_left: Point::new(12, 4), size: Size::new(4, 4) };

#[test]
fn span_pairing_full_present() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn span_pairing_damage_translation() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_damage_translation(&mut frame, &mut p, rgb(RED), rgb(BLUE), true));
}

#[test]
fn span_pairing_overlay_backdrop() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_backdrop(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        SPAN_RECT,
        (13, 5),
        (14, 6),
        |f| f.bytes().to_vec(),
    ));
}

#[test]
fn span_pairing_overlay_exclusion() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_exclusion(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        rgb(GREEN),
        SPAN_RECT,
        (13, 5),
        (14, 6),
        (0, 0),
        |f| f.bytes().to_vec(),
        true,
    ));
}

#[test]
fn span_pairing_overlay_pop_retract_clear() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_pop_retract_clear(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        SPAN_RECT,
        (12, 5),
        (15, 6),
        |f| f.bytes().to_vec(),
        true,
    ));
}

// The proof pairing's probe geometry: the right half of the 16×8 frame = tiles {2, 3, 6, 7}; a
// point in tile 0 is outside the widened region.
const TILE_RECT: Rectangle = Rectangle { top_left: Point::new(8, 0), size: Size::new(8, 8) };

#[test]
fn tile_pairing_full_present() {
    let mut frame = MiniFrame { cells: [0; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn tile_pairing_damage_translation() {
    let mut frame = MiniFrame { cells: [0; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(conformance::check_damage_translation(&mut frame, &mut p, rgb(RED), rgb(BLUE), true));
}

#[test]
fn tile_pairing_overlay_backdrop() {
    let mut frame = MiniFrame { cells: [0; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(conformance::check_overlay_backdrop(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        TILE_RECT,
        (10, 2),
        (13, 5),
        |f| f.cells,
    ));
}

#[test]
fn tile_pairing_overlay_exclusion() {
    let mut frame = MiniFrame { cells: [0; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(conformance::check_overlay_exclusion(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        rgb(GREEN),
        TILE_RECT,
        (10, 2),
        (13, 5),
        (0, 0),
        |f| f.cells,
        true,
    ));
}

#[test]
fn tile_pairing_overlay_pop_retract_clear() {
    let mut frame = MiniFrame { cells: [0; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(conformance::check_overlay_pop_retract_clear(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        TILE_RECT,
        (9, 1),
        (14, 6),
        |f| f.cells,
        true,
    ));
}

/// The padded stride is real: a full-frame clear through the contract's draw view must leave the
/// 4 padding cells of every row untouched.
#[test]
fn mini_frame_padded_stride_cells_stay_untouched() {
    let mut f = MiniFrame { cells: [0xAAAA; MSTRIDE * MH] };
    f.draw_target().clear(rgb(0)).unwrap();
    for y in 0..MH {
        assert_eq!(f.cells[y * MSTRIDE + MW..(y + 1) * MSTRIDE], [0xAAAA; MSTRIDE - MW], "row {y} padding");
        assert_eq!(f.cells[y * MSTRIDE], 0, "row {y} pixels cleared");
    }
}

/// The tile overlay path never writes the frame at all — the strongest form of "transient overlays
/// never become persistent framebuffer contents". (The span pairing's equivalent is checked below.)
#[test]
fn overlay_leaves_the_frame_byte_identical() {
    // Tile pairing: bounded-scratch composite, frame untouched by construction — verify anyway.
    let mut frame = MiniFrame { cells: [0x1234; MSTRIDE * MH] };
    let mut p = TilePresenter::new();
    block_on(p.present(&frame, TilePresenter::damage_full())).unwrap();
    let before = frame.cells;
    let region = <TilePresenter as OverlayPresenter<MiniFrame>>::region(TILE_RECT);
    block_on(p.present_overlay(&mut frame, region, |t: &mut MiniOverlay| {
        let _ = t.fill_solid(&Rectangle::new(Point::new(10, 2), Size::new(2, 2)), rgb(BLUE));
    }))
    .unwrap();
    assert_eq!(frame.cells, before);

    // Span pairing: composite goes through scratch + glass; the resident bytes stay the clean map.
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    frame.draw_target().clear(rgb(RED)).unwrap();
    block_on(p.present(&frame, SpanPresenter::<16, 16>::damage_full())).unwrap();
    let before = frame.bytes().to_vec();
    let region = <SpanPresenter<16, 16> as OverlayPresenter<Device64Frame<16, 16>>>::region(SPAN_RECT);
    block_on(p.present_overlay(&mut frame, region, |band: &mut Band| {
        let _ = band.fill_solid(&Rectangle::new(Point::new(13, 5), Size::new(1, 1)), rgb(BLUE));
    }))
    .unwrap();
    assert_eq!(frame.bytes(), &before[..]);
}

// ─── The legacy-seam bridge over pairing 1 ───

/// Drive the old `DisplayDriver` call shapes — `fb_mut` through `&mut dyn`, `present(exclude)`,
/// `present_overlay(OverlayRegion, Band drawer)`, and the `false` transport outcome — over the
/// (frame, presenter) pairing, proving the old seam is a special case of the contracts.
#[test]
fn bridged_driver_runs_the_old_seam_over_the_pairing() {
    let mut buf = [0u8; 16 * 16];
    let mut d = BridgedDriver::new(Device64Frame::<16, 16>::new(&mut buf), SpanPresenter::<16, 16>::new());
    let glass_at = |d: &BridgedDriver<Device64Frame<16, 16>, SpanPresenter<16, 16>>, x: u32, y: u32| {
        GlassProbe::<Device64Frame<16, 16>>::glass(d.presenter(), x, y)
    };

    // Render through the object-safe half of the old seam, exactly like the app loop.
    {
        let dyn_d: &mut dyn DisplayDriver = &mut d;
        let mut fb = FbDevice64::new(dyn_d.fb_mut(), 16, 16);
        fb.clear(rgb(RED)).unwrap();
    }
    assert!(block_on(d.present(None)), "a clean present reports true");
    let g_red = glass_at(&d, 13, 5);

    // The hold bulge over the clean backdrop, via the old Band-typed drawer.
    let ok = block_on(d.present_overlay(OverlayRegion { x0: 12, y0: 4, w: 4, rows: 4 }, &mut |band: &mut Band| {
        band.fill_solid(&Rectangle::new(Point::new(13, 5), Size::new(1, 1)), rgb(BLUE)).ok();
    }));
    assert!(ok);
    let g_bulge = glass_at(&d, 13, 5);
    assert_ne!(g_bulge, g_red, "the bulge reached glass");

    // A map redraw presents around the live bulge: its rows are excluded, the rest updates.
    {
        let mut fb = FbDevice64::new(d.fb_mut(), 16, 16);
        fb.clear(rgb(GREEN)).unwrap();
    }
    assert!(block_on(d.present(Some((4, 4)))));
    assert_eq!(glass_at(&d, 13, 5), g_bulge, "exclude went around the live bulge");
    assert_ne!(glass_at(&d, 0, 0), g_red, "the rest of the frame updated");

    // The trailing clear: re-present the bulge rows with nothing composited.
    assert!(block_on(d.present_overlay(OverlayRegion { x0: 12, y0: 4, w: 4, rows: 4 }, &mut |_band: &mut Band| {})));
    assert_eq!(glass_at(&d, 13, 5), glass_at(&d, 0, 0), "the clean frame is restored under the bulge");

    // A transport fault surfaces as the old seam's `false`.
    d.presenter_mut().fail_next = true;
    assert!(!block_on(d.present(None)));
}
