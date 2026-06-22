//! STM32F429I-DISC1 board firmware for the OpenBikeComputer hardware prototype.
//!
//! This is the on-glass stand-in while the real target (nRF54L + MIP panel) is in
//! development: it brings the shared `obc-app` up on a real display + buttons + SD so
//! the HAL seams become concrete and the nRF firmware is a port, not a rewrite. The
//! LTDC + ILI9341 path here is specific to this Discovery board; nothing app-facing
//! depends on it.
//!
//! Bring-up is phased so each hardware layer is verified over defmt/RTT before the
//! next is stacked:
//!   A. clocks + RTT + GPIO              [done]
//!   B. FMC SDRAM (8 MB @ 0xD0000000)    [done: 8 MB verified, 0 errors]
//!   C. ILI9341 (SPI5) + LTDC + SDRAM framebuffer + test pattern   [done: issue #33]
//!   D. obc-app on glass (issue #34): the SDRAM framebuffer becomes an
//!      `embedded-graphics` `DrawTarget`, and the shared `App` (boots to Home/Idle,
//!      then opens the Map on a baked-in OBCM tile) is rendered through it via
//!      `App::render_frame` — the first time `obc-app` runs on hardware.
//!   E. Pushbutton input (issue #35): four GPIO buttons → obc-platform's
//!      board-agnostic `ButtonInput` debouncer → the shared gesture recognizer, so
//!      the UI is driven on-device — Home → Route menu → Map and back.
//!   F. microSD over SPI (issue #36): SPI4 + FatFs (`embedded-sdmmc`) → the card's map
//!      `.obcm` loads resident, the `/routes/*.obcr` catalog fills the Route menu, the
//!      chosen route streams from the card, and the ride logs to `/tracks` and saves as a
//!      `.gpx`. The FatFs byte adapters live in `obc-platform` (shared with the nRF
//!      board); only the SPI bus + chip-select here are board-specific.   <- this commit
//!
//! ## RAM split (issue #34 / #8)
//! The renderer's per-frame scratch is ~200 KB and the framebuffer is 150 KB; the
//! two do not both fit the F429's 192 KB internal SRAM. For the prototype the whole
//! `App` (which embeds the renderer) is placed in **SDRAM**, just past the four
//! framebuffers (the double-buffered map plane + the double-buffered Layer 2 overlay,
//! 4x150 KB — see below) — simplest, runs the full-size renderer. The 8 MB SDRAM swallows
//! all of it. The cost is render-time:
//! the scratch is now behind the FMC's wait states (slower than the internal-RAM
//! `mcu-render-bench`); the per-frame time logged over RTT quantifies the delta.
//! A `small-scratch` cargo feature (internal-RAM scratch) is the fallback if that
//! delta ever matters — not needed yet.
//!
//! ## Double buffering (the map plane)
//! Rendering clears and repaints the whole frame, so drawing straight into the buffer
//! the LTDC is scanning makes the panel flash on every redraw — fine for a static
//! demo, but ugly once the map changes. So the app path keeps **two** map framebuffers:
//! the LTDC scans the *front* while the app renders the next frame into the *back*, then
//! [`flip_to`] points the layer at the back and reloads it at the next vertical blank
//! (tear-free) and the roles swap. The panel only ever shows a fully-rendered map frame.
//! (The `glass-demo` path stays single-buffered, single-layer — it draws one static
//! screen and halts.)
//!
//! ## Dual-layer display (issue #46): map plane + overlay plane
//! The LTDC has two blendable layers. The double-buffered map is **Layer 1** (the bottom,
//! opaque RGB565 plane). The transient UI chrome — the hold-bulge / confirm ring
//! ([`obc_app`]'s overlay plane, issue #45) — renders into **Layer 2**, an ARGB4444
//! framebuffer the LTDC composites over Layer 1 in hardware (`BC = α·overlay + (1−α)·map`).
//! So when only the ring animates (the common case: a static map under a charging hold),
//! **only Layer 2 repaints** — the map is never re-rendered, and an overlay frame (a
//! transparent buffer clear + a few bulge strips) is a couple of ms at most vs. tens of ms
//! for a map frame (the RTT `overlay frame` / `map frame` logs quantify both).
//!
//! Layer 2 is **double-buffered** exactly like the map, for the same reason: the overlay
//! redraws the whole buffer (clear-to-transparent then the bulge), so writing in place into
//! the buffer the LTDC is scanning tears — the bulge visibly flickers as the clear races the
//! scan (an in-place single buffer was the first cut; it flickered on glass). Instead the app
//! renders the next overlay frame into the *back* overlay buffer and [`flip_to`]s Layer 2 at
//! the vblank, tear-free, then swaps — so the panel only ever scans a complete overlay frame.
//! That makes **four** framebuffers in SDRAM (2 map + 2 overlay, 4x150 KB ≈ 600 KB), still ≪
//! the 8 MB SDRAM (the issue sketched a single overlay buffer; double-buffering is the fix the
//! observed tearing demanded).
//!
//! ## Render-on-demand (issue #47): dirty-region tracking
//! The render loop is driven by the shared app's **dirty signal**, not a clock. As the `App`
//! ticks sensors and handles input it accumulates *which planes changed*; `App::take_dirty`
//! reports `{ map, overlay }` once per frame, and the loop re-renders **Layer 1 only when
//! `map`** and **Layer 2 only when `overlay`**. So a genuinely static Map performs **zero** map
//! renders (the old design forced a full 24–51 ms map frame every 1 s purely to keep time-based
//! screens live — wasteful on the MIP/battery target this prototypes). The map redraws on a
//! gesture, a camera-moving GPS fix, a route load, or a screen's own timed content (the
//! Statistics cursor spring-back, which the screen surfaces via `Screen::animate`); the overlay
//! repaints whenever the hold bulge is live. Buttons are still sampled every `LOOP_MS` so taps
//! and hold timing are never missed — only *rendering* is on-demand. Map-matching and the ride
//! log run per fresh fix, decoupled from the render cadence (the route reader is opened every
//! frame — it does no I/O until geometry is actually streamed).
//!
//! ## Two-plane preemptive input/overlay (issue #48)
//! Dirty-tracking cuts *how often* the map renders, but a `render_map` call still blocks its
//! executor for its whole 24–51 ms — so input goes sluggish *during* a render (panning
//! re-renders rapidly while a button is held). The fix is to run input + the overlay on a
//! **high-priority `InterruptExecutor`** (pended from the unused **UART5** vector at **P6** —
//! above thread mode so it preempts the map render, below the P0 embassy-time driver so its
//! `Timer`s still wake mid-render) and the `App` + map render on the thread-mode executor:
//!   - **High-priority plane** ([`input_overlay_task`]): owns the `ButtonInput`, the shared
//!     `obc_app::InputPlane` (recogniser + hold-hint overlay), and the double-buffered **Layer 2**
//!     overlay. Every `LOOP_MS` it samples the buttons, recognises gestures — pushing each into
//!     the [`GESTURES`] channel — and repaints + flips the bulge on Layer 2. So the confirm bulge
//!     stays at full FPS and the auto-repeat cadence stays exact *while* the map re-renders.
//!   - **Map plane** (this `main` loop, thread mode): drains [`GESTURES`] → `App::apply_gesture`,
//!     `App::advance_animations`, ticks sensors, and re-renders **Layer 1** on `dirty.map`.
//!
//! The only shared state is the lock-free gesture channel + the two disjoint framebuffers, so
//! the long map render holds no lock against input. The bulge confirms a press instantly; the
//! screen transition lands a frame later when the map plane drains the channel. The shared
//! LTDC vblank-reload bit (both planes flip their own layer) is guarded by a short critical
//! section in [`flip_to`]. A `single-executor` cargo feature drops back to the old one-loop
//! path (`App::handle_input` + inline overlay) to prove the `InputPlane`/`apply_gesture` seam
//! composes; the two-plane split is the default and the structure `obc-fw-nrf54l` will reuse.
//!
//! Clock: 180 MHz core from the 16 MHz HSI (no dependency on the DISC1 HSE), plus a
//! PLLSAI leg for the LTDC pixel clock.
//!
//! ## SDRAM pin map (FMC bank 2, IS42S16400J, 8 MB @ 0xD000_0000)
//! Address  A0-A11 : PF0 PF1 PF2 PF3 PF4 PF5 PF12 PF13 PF14 PF15 PG0 PG1
//! Bank     BA0-1  : PG4 PG5
//! Data     D0-D15 : PD14 PD15 PD0 PD1 PE7 PE8 PE9 PE10 PE11 PE12 PE13 PE14 PE15 PD8 PD9 PD10
//! Mask     NBL0-1 : PE0 PE1
//! Control         : SDCKE1 PB5 | SDCLK PG8 | SDNCAS PG15 | SDNE1 PB6 | SDNRAS PF11 | SDNWE PC0
//!
//! ## Pushbutton pin map (issue #35) — DISC1 headers, internal pull-ups, active-low
//! One common pin to GND; each switch to its GPIO (no external pull-ups/resistors —
//! the F429's internal pull-ups hold the lines high, a press pulls one low):
//!   PREV PD4 | NEXT PE3 | SELECT PE4 | BACK PD5
//! Clear of FMC/LTDC/SPI5/LEDs, and deliberately kept off SPI4's data pins so the SD
//! card (#36 uses SD-over-**SPI** / embedded-sdmmc, not SDIO) can take SPI4. PE4 =
//! SPI4_NSS is still SELECT, but SPI-mode SD uses a software CS, so it never needs
//! hardware NSS. PD4/PD5 are free GPIO broken out on the headers. PREV/BACK were moved
//! off PE2/PE5 for exactly this.
//!
//! ## microSD pin map (issue #36) — SPI4, chip-select held low, internal MISO pull-up
//! SPI4's only usable data pinout on the DISC1 (PE11-14 is all FMC): SCK PE2 / MISO PE5
//! / MOSI PE6. CS is a free GPIO (PD7), held LOW for the whole session — embassy's SPI
//! can't tolerate embedded-sdmmc toggling CS between a command and its reply (the card
//! drops the bus and CMD0's response is lost), so a no-op CS is used instead (see
//! `sd::NoCs` / `sd::init`). Wire the breakout: SCK→PE2, MOSI→PE6, MISO→PE5, CS→PD7,
//! GND→GND, VCC→3V3 (bare socket) or 5V (regulated breakout). Card is FAT16/FAT32, init
//! at ≤400 kHz then re-clocked (see `sd::init`).
//!   SCK PE2 | MISO PE5 | MOSI PE6 | CS PD7
//!
//! ## Display pin map (onboard ILI9341, 240x320)
//! Config (SPI5, 8-bit mode-0) : SCK PF7 | MOSI PF9 | CS/NCS PC2 | DCX/WRX PD13   (reset = NRST)
//! LTDC sync                   : HSYNC PC6 | VSYNC PA4 | CLK PG7 | DE PF10
//! LTDC RGB666 (R0/R1,G0/G1,B0/B1 not wired) — AF14 unless noted AF9:
//!   R2 PC10  R3 PB0*  R4 PA11  R5 PA12  R6 PB1*  R7 PG6
//!   G2 PA6   G3 PG10* G4 PB10  G5 PB11  G6 PC7   G7 PD3
//!   B2 PD6   B3 PG11  B4 PG12* B5 PA3   B6 PB8   B7 PB9        (* = AF9)
//! (all per ST's 32f429idiscovery-bsp; IS42S16400J + ILI9341 timings from there too.)

#![no_std]
#![no_main]

use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_stm32::fmc::Fmc;
use embassy_stm32::gpio::{AfType, Flex, Level, Output, OutputType, Pin, Speed};
use embassy_stm32::ltdc::{
    Ltdc, LtdcConfiguration, LtdcLayer, LtdcLayerConfig, PixelFormat, PolarityActive, PolarityEdge,
};
use embassy_stm32::spi::Spi;
use embassy_stm32::time::Hertz;
use embassy_stm32::Peri;
use embassy_time::{Delay, Timer};
use obc_platform::Framebuffer565;
use panic_probe as _;

// The shared-app render path — only the `not(glass-demo)` build drives it (the demo
// just exercises the framebuffer + text). Kept behind the cfg so the demo build is
// warning-free too.
#[cfg(not(feature = "glass-demo"))]
use embassy_stm32::gpio::{Input, Pull};
#[cfg(not(feature = "glass-demo"))]
use embassy_time::{Duration, Instant};
#[cfg(not(feature = "glass-demo"))]
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
#[cfg(not(feature = "glass-demo"))]
use obc_app::{App, AppState, InputClock, InputEvent, InputSource, RideClock, RouteSummary, Sensors, TrackSink};
// Only the `debug-usb`-off fallback implements its own `LocationSource` (`SynthLocation`); the
// USB build's sources live in obc-platform, so these would be unused there.
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
use obc_app::{Fix, LocationSource};
#[cfg(not(feature = "glass-demo"))]
use obc_platform::{ButtonInput, FramebufferArgb4444};
#[cfg(not(feature = "glass-demo"))]
use obc_reader::{BBox, ByteSource, MapCache, Reader};
// `SliceSource` only wraps the baked-in tile; the SD path streams through `SdByteSource`.
// Gated like its use sites in the real-app path (baked-tile, and not the glass-demo build, which
// has no map loading) so `--all-features` (which turns on both) doesn't see it as unused.
#[cfg(all(feature = "baked-tile", not(feature = "glass-demo")))]
use obc_reader::SliceSource;
#[cfg(not(feature = "glass-demo"))]
use obc_render::zoom_for_mpp;
#[cfg(not(feature = "glass-demo"))]
use obc_route::{RouteIndex, RouteReader};

// The two-plane (default) build only: the high-priority interrupt executor that runs the
// input/overlay plane, the lock-free gesture channel feeding the map plane, and the
// shared `InputPlane`/`Gesture` it carries. The `single-executor` fallback drives input
// inline through `App::handle_input` instead, so none of this is compiled there.
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use embassy_executor::InterruptExecutor;
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use embassy_stm32::interrupt;
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use embassy_stm32::interrupt::{InterruptExt, Priority};
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use embassy_sync::channel::{Channel, Receiver, Sender};
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
use obc_app::{Gesture, InputPlane};

#[cfg(feature = "glass-demo")]
mod demo;
// microSD map/route/track storage over SPI4 + FatFs (issue #36) — only the real-app build
// touches the card; the glass demo just exercises the framebuffer.
#[cfg(not(feature = "glass-demo"))]
mod sd;

/// Panic-free `Instant::elapsed()` (issue #51). embassy-time's own `Instant::elapsed()` is
/// `Instant::now() - *self`, and `Instant - Instant` calls `duration_since`, which `unwrap!`s a
/// `checked_sub` — so it **panics** the instant `now()` reads *less* than the captured instant.
/// `now()` doing exactly that (a momentarily non-monotonic read) is a known embassy-stm32
/// time-driver race when the 16-bit hardware timer is extended to a 64-bit tick count, and the
/// panic `udf`s → HardFault → the board halts. Every `.elapsed()` in this firmware (the frame
/// `now`, the flip-reload timeouts, the render-stat deltas) only wants "how long since", and a
/// transient backwards read meaning "zero time passed" is harmless to all of them — so clamp to
/// zero via `saturating_duration_since` instead of panicking the device.
#[cfg(not(feature = "glass-demo"))]
trait SaturatingElapsed {
    fn saturating_elapsed(&self) -> Duration;
}

#[cfg(not(feature = "glass-demo"))]
impl SaturatingElapsed for Instant {
    fn saturating_elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(*self)
    }
}

// --- USB-CDC fake sensors (issue #38, behind `debug-usb`) ---
// The DISC1 has no GPS/baro/compass, so a host streams a recorded ride over the USER USB port
// (OTG_HS internal full-speed PHY on PB14/PB15) and `obc-platform`'s debug sources turn it into
// the HAL traits the app already polls. embassy-stm32 owns the OTG `Driver`; the protocol +
// sources live in obc-platform so they move to the nRF unchanged. Three small async tasks on the
// thread-mode executor: the device stack, the line-RX → sensor-signal pump, and the ~1 Hz
// telemetry TX. OTG RX is interrupt-buffered, so the 24–51 ms map render never drops bytes.
#[cfg(feature = "debug-usb")]
use embassy_stm32::{bind_interrupts, peripherals, usb};
// Aliased so the CDC endpoints don't clash with embassy-sync's channel `Sender`/`Receiver`
// (imported by the two-plane build for the gesture channel).
#[cfg(feature = "debug-usb")]
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver as CdcReceiver, Sender as CdcSender, State};

#[cfg(feature = "debug-usb")]
bind_interrupts!(struct UsbIrqs {
    OTG_HS => usb::InterruptHandler<peripherals::USB_OTG_HS>;
});

/// The concrete OTG full-speed driver the CDC tasks carry (`USB_OTG_HS` + internal FS PHY).
#[cfg(feature = "debug-usb")]
type UsbDriver = usb::Driver<'static, peripherals::USB_OTG_HS>;

/// Run the USB device stack (enumeration, control transfers). Spawned once; never returns.
#[cfg(feature = "debug-usb")]
#[embassy_executor::task]
async fn usb_device_task(mut device: embassy_usb::UsbDevice<'static, UsbDriver>) {
    device.run().await
}

/// CDC RX → sensor signals: accumulate received bytes into lines and dispatch each `F`/`A`/`C`
/// into `obc-platform`'s fresh-fix signals, which the app's `DebugLocation`/`DebugAltimeter`/
/// `DebugCompass` poll. Re-arms on disconnect/reconnect.
#[cfg(feature = "debug-usb")]
#[embassy_executor::task]
async fn usb_rx_task(mut rx: CdcReceiver<'static, UsbDriver>) {
    let mut buf = [0u8; 64];
    loop {
        rx.wait_connection().await;
        // A fresh reader per session: a partial line buffered when the previous session
        // disconnected must not be prepended to (and corrupt) this session's first line.
        let mut reader = obc_platform::debug_usb::LineReader::new();
        // Loop until the packet read errors (disconnect) — then wait for the next connection.
        while let Ok(n) = rx.read_packet(&mut buf).await {
            obc_platform::debug_usb::feed_bytes(&mut reader, &buf[..n]);
        }
    }
}

/// CDC TX ← telemetry: send one compact status line each time the app publishes telemetry
/// (~1 Hz via `set_telemetry`), so the host's readout updates without the device ever polling or
/// flooding the link. Re-arms on disconnect/reconnect.
#[cfg(feature = "debug-usb")]
#[embassy_executor::task]
async fn usb_tx_task(mut tx: CdcSender<'static, UsbDriver>) {
    loop {
        tx.wait_connection().await;
        loop {
            let t = obc_platform::debug_usb::wait_telemetry().await;
            let line = obc_platform::debug_usb::format_telemetry(&t);
            if tx.write_packet(line.as_bytes()).await.is_err() {
                break; // disconnected — wait for the next connection
            }
        }
    }
}

/// Chains two input sources for the gesture recogniser: drains `a` (the physical buttons) fully,
/// then `b` (the USB-injected events with `debug-usb`, else [`NullInput`]). So the recogniser sees
/// injected turns/edges interleaved with real presses and turns them into gestures identically.
#[cfg(not(feature = "glass-demo"))]
struct ChainedInput<'a> {
    a: &'a mut dyn InputSource,
    b: &'a mut dyn InputSource,
}
#[cfg(not(feature = "glass-demo"))]
impl InputSource for ChainedInput<'_> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.a.poll().or_else(|| self.b.poll())
    }
}

/// An input source that never yields — the `debug-usb`-off stand-in for the USB-injected stream,
/// so the recogniser call site is one code path in both builds.
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
struct NullInput;
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
impl InputSource for NullInput {
    fn poll(&mut self) -> Option<InputEvent> {
        None
    }
}

const SDRAM_ADDR: usize = 0xD000_0000;
/// Front framebuffer at the base of SDRAM: 240x320 RGB565 = 150 KB. The app path adds a
/// second (back) buffer one [`FB_BYTES`] past it for double buffering; the LTDC is first
/// pointed here at init and thereafter flipped between the two by [`flip_to`].
const FB_ADDR: usize = SDRAM_ADDR;
const W: usize = 240;
const H: usize = 320;
/// Framebuffer extent in pixels / bytes (RGB565, 2 bytes each).
const FB_PIXELS: usize = W * H;
/// Only the app path needs this — the back-buffer offset, and it places the overlay
/// framebuffer + the `App` in SDRAM just past the map framebuffers.
#[cfg(not(feature = "glass-demo"))]
const FB_BYTES: usize = FB_PIXELS * 2;

/// Layer 2 overlay framebuffers (issue #46): the *front* of a double-buffered ARGB4444
/// pair (2 B/px → the same [`FB_BYTES`] as an RGB565 map buffer each), placed just past the
/// two double-buffered map framebuffers; the *back* sits one [`FB_BYTES`] further on. The
/// LTDC blends the front over the map and the app renders the next overlay frame into the
/// back, then [`flip_to`]s Layer 2 at the vblank and swaps (tear-free, like the map plane).
#[cfg(not(feature = "glass-demo"))]
const OVERLAY_ADDR: usize = SDRAM_ADDR + 2 * FB_BYTES;

/// Total FMC SDRAM the IS42S16400J provides (8 MB @ [`SDRAM_ADDR`]).
#[cfg(not(feature = "glass-demo"))]
const SDRAM_BYTES: usize = 8 * 1024 * 1024;

// Build-time guard for the resident SDRAM placement (issue #67): the four framebuffers
// (2 map + 2 overlay), the `App` slot, and the `MapCache` must all fit the 8 MB. `App` is
// initialized *in place* by `App::init_idle` — never returned by value into a stack temporary —
// so the only way it can break the placement is by outgrowing this region; this fails the build
// if it (or `MapCache`, or the framebuffers) ever does, instead of corrupting SDRAM on glass.
// (Per-region alignment padding is a handful of bytes against ~7.5 MB of headroom; ignored here.)
#[cfg(not(feature = "glass-demo"))]
const _: () = assert!(
    4 * FB_BYTES + core::mem::size_of::<App>() + core::mem::size_of::<MapCache>() <= SDRAM_BYTES,
    "resident SDRAM set (framebuffers + App + MapCache) overruns the 8 MB SDRAM"
);

/// Baked-in OBCM **v5** map tile (issue #34): a small ~1.4 MB Teningen tile in
/// flash via `include_bytes!`. With #36 the map normally comes off the SD card; this stays
/// as the **fallback** when no card / no `.obcm` is present, so the device still boots to a
/// usable Map. Packed from `packer/small.obcm`. Behind the `baked-tile` feature (default on);
/// `--no-default-features` drops it for a tiny, fast-to-flash image while iterating.
#[cfg(all(not(feature = "glass-demo"), feature = "baked-tile"))]
static TILE: &[u8] = include_bytes!("../tiles/teningen.obcm");

/// Initial camera zoom, in ground **metres-per-pixel** (in the 0.5–4 mpp riding band).
/// Used for the Idle [`AppState`]; opening the Map via the Route menu resets to the
/// riding zoom, and PREV/NEXT then zoom from there.
#[cfg(not(feature = "glass-demo"))]
const INIT_MPP: f32 = 1.0;

/// Stand-in moving GPS — the **`debug-usb`-off fallback** (the default build streams a real ride
/// over USB instead, see issue #38): side length (m) and speed (m/s) of the square loop
/// [`SynthLocation`] walks. Slow enough to watch the user marker / breadcrumb crawl, big enough
/// that a saved ride is a real ~0.8 km loop that re-imports as a sane route.
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
const SYNTH_LEG_M: f32 = 200.0;
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
const SYNTH_SPEED_MPS: f32 = 5.0;

/// The synthetic GPS emits a fresh fix at this cadence (ms), `None` between — so the prototype
/// drives the app on the same ~1 Hz fresh-fix contract a real receiver (and the USB feed)
/// honours, exercising the integrate-one-sample path instead of an every-tick replay (#43).
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
const SYNTH_FIX_INTERVAL_MS: u64 = 1000;

/// Microdegrees of latitude per metre north (the map/route coordinate convention). Longitude
/// scales this by 1/cos(lat), via [`obc_route::cos_lat`].
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
const UDEG_PER_M: f32 = 1_000_000.0 / 111_320.0;

/// Main-loop button-sample period (ms). Buttons are sampled every tick so quick taps
/// and hold timing are caught even when a heavy Map frame stretches the render cadence.
/// On the two-plane build this is the **input plane's** period — sampled on the
/// high-priority executor that preempts the map render, so the cadence is exact regardless
/// of how long a map frame takes; the map plane polls the gesture channel on the same period.
#[cfg(not(feature = "glass-demo"))]
const LOOP_MS: u64 = 8;

// --- two-plane architecture (issue #48): the high-priority input/overlay plane ---
//
// The map plane (this crate's `main`) runs on the thread-mode executor at the lowest
// priority; `render_map` is a 24–51 ms CPU-bound call that blocks it. To keep input + the
// overlay responsive *during* that render, the input plane runs on an `InterruptExecutor`
// pended from an unused IRQ at a priority above thread mode — so it preempts the map render
// every `LOOP_MS`, samples the buttons, recognises gestures (pushing each into the channel
// below) and animates the Layer-2 bulge on its own. The embassy-time driver IRQ stays more
// urgent still (it runs at the reset-default priority), so the input plane's `Timer`s wake
// even mid-render. This is embassy's documented multi-priority pattern.

/// Bound of the gesture channel from the input plane to the map plane. One frame yields at
/// most a couple of gestures and the map plane drains it each loop, so even across a ~50 ms
/// map render (a handful of input samples) it never fills — the same slack as `ButtonInput`'s
/// queue. `try_send` simply drops on the (unreachable) overflow rather than ever blocking the
/// high-priority plane.
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane (high priority) to the map plane
/// (thread mode). The only shared state between the two planes — lock-free, so the long map
/// render never holds anything against the input plane.
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// The high-priority executor the input/overlay plane runs on. Started in `main` and driven
/// by the [`UART5`](input_plane_irq) interrupt handler (UART5 is unused on this board, so its
/// vector is free to repurpose as the executor's software-pend line).
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
static EXECUTOR_INPUT: InterruptExecutor = InterruptExecutor::new();

/// UART5 ISR → poll the input-plane executor. The peripheral itself is unused; we only
/// borrow its interrupt vector as the executor's pend line (set its priority + start the
/// executor in `main`).
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
#[interrupt]
unsafe fn UART5() {
    EXECUTOR_INPUT.on_interrupt();
}

/// The input + overlay plane task. Runs on [`EXECUTOR_INPUT`], preempting the map render:
/// every `LOOP_MS` it samples the buttons, recognises gestures (pushing each into `gestures`
/// for the map plane to apply), and — when the hold bulge changes — repaints the
/// double-buffered Layer-2 overlay and vblank-flips it, all without touching anything the map
/// plane owns. So press-to-feedback latency and the auto-repeat cadence stay bounded no matter
/// how long a map frame takes.
#[cfg(all(not(feature = "glass-demo"), not(feature = "single-executor")))]
#[embassy_executor::task]
async fn input_overlay_task(
    mut buttons: ButtonInput<Input<'static>>,
    mut plane: InputPlane,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
    mut overlay_front: usize,
    mut overlay_back: usize,
    start: Instant,
) {
    // Native RGB565 panel → identity colour map, same as the map plane (see `main`).
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
    loop {
        let now = start.saturating_elapsed().as_millis() as u32;
        buttons.update(now);
        // Physical buttons + (with `debug-usb`) the USB-injected events, drained into one
        // recogniser pass — so a host can drive the UI (taps/holds) like the real buttons.
        #[cfg(feature = "debug-usb")]
        let mut usb_in = obc_platform::debug_usb::DebugInput;
        #[cfg(not(feature = "debug-usb"))]
        let mut usb_in = NullInput;
        let mut input = ChainedInput { a: &mut buttons, b: &mut usb_in };
        // Recognise this frame's input on the *input* clock (wall time, preemptive). Each
        // gesture is pushed to the map plane; the bulge is animated below regardless, so the
        // press is confirmed on screen immediately even before the map plane drains it.
        plane.recognize(InputClock(now), &mut input, |g| {
            if gestures.try_send(g).is_err() {
                defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
            }
        });

        // Repaint the overlay only when the bulge changed (plus the one trailing clear). Like
        // the map plane it is double-buffered + vblank-flipped, so the bulge never tears.
        if plane.take_overlay_dirty() {
            // SAFETY: overlay_back is the Layer-2 buffer the LTDC is *not* scanning; the only
            // live `&mut` over it, dropped before the flip.
            let back = unsafe { core::slice::from_raw_parts_mut(overlay_back as *mut u16, FB_PIXELS) };
            let mut overlay_fb = FramebufferArgb4444::new(back, W as u32, H as u32);
            overlay_fb.clear_transparent();
            plane.render_overlay(&mut overlay_fb, W as f32, H as f32, color_fn);
            // Arm the flip, then `await` the vblank by yielding ~1 ms at a time — this runs in the
            // high-priority ISR, so busy-polling here would hold off the very map render we just
            // preempted; yielding instead lets the thread-mode map plane keep rendering while we
            // wait. Swap the buffers only once the reload lands (tear-free, same contract as
            // `flip_to`); on the rare timeout keep them and retry next frame.
            request_flip(LtdcLayer::Layer2, overlay_back);
            let t0 = Instant::now();
            let landed = loop {
                if flip_landed() {
                    break true;
                }
                if t0.saturating_elapsed().as_millis() > 50 {
                    break false;
                }
                Timer::after_millis(1).await;
            };
            if landed {
                core::mem::swap(&mut overlay_front, &mut overlay_back);
            } else {
                defmt::warn!("LTDC: Layer 2 vblank reload didn't land in 50 ms — kept buffers, skipped swap");
            }
        }
        Timer::after_millis(LOOP_MS).await;
    }
}

/// A stand-in moving [`LocationSource`] for the **`debug-usb`-off** build (the default streams a
/// real GPS over USB-CDC, issue #38): the fix walks a slow square loop around a centre, driven by
/// the wall clock. Unlike a constant fix, this gives the ride accumulators, breadcrumb and `.obct`
/// log real motion — so a saved ride is a non-degenerate `.gpx` that re-imports cleanly (issue
/// #36's save-loop deliverable). The centre is the map (or loaded route's) start, re-pointed via
/// [`recenter`](Self::recenter).
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
struct SynthLocation {
    center_lon: i32,
    center_lat: i32,
    /// 1/cos(lat) folded into the east-metres → microdegrees scale, refreshed on recenter.
    udeg_per_m_east: f32,
    start: Instant,
    /// Elapsed-millis at the last fix [`poll`](LocationSource::poll) emitted, to throttle to
    /// [`SYNTH_FIX_INTERVAL_MS`]. `None` forces the first poll to emit.
    last_fix_ms: Option<u64>,
}

#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
impl SynthLocation {
    fn new(center_lon: i32, center_lat: i32, start: Instant) -> Self {
        let mut s = SynthLocation { center_lon, center_lat, udeg_per_m_east: 0.0, start, last_fix_ms: None };
        s.recenter(center_lon, center_lat);
        s
    }

    /// Move the loop's centre (e.g. onto a freshly-loaded route's start) and refresh the
    /// longitude scale for the new latitude.
    fn recenter(&mut self, lon: i32, lat: i32) {
        self.center_lon = lon;
        self.center_lat = lat;
        self.udeg_per_m_east = UDEG_PER_M / obc_route::cos_lat(lat);
    }
}

#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-usb")))]
impl LocationSource for SynthLocation {
    fn poll(&mut self) -> Option<Fix> {
        // Emit on the GPS's own ~1 Hz cadence, `None` between — the exact fresh-fix contract a
        // real receiver (and #38's USB feed) honours, so the prototype walks the same
        // integrate-one-sample path rather than the every-tick replay that masked issue #43.
        let elapsed_ms = self.start.saturating_elapsed().as_millis();
        if let Some(last) = self.last_fix_ms {
            if elapsed_ms.wrapping_sub(last) < SYNTH_FIX_INTERVAL_MS {
                return None;
            }
        }
        self.last_fix_ms = Some(elapsed_ms);

        // Position along the square as a function of elapsed time. Each leg takes `leg_s`
        // seconds; the heading is the leg's constant bearing (no trig needed). The loop is
        // centred on the square so the camera sits in its middle. Take the loop modulus on the
        // integer millis *before* the `f32` cast: `as_millis()` grows without bound and `f32`
        // carries only a 24-bit mantissa, so casting first would quantise the phase (the loop
        // would jitter, then freeze) once the board had been up past ~4.6 h.
        let leg_s = SYNTH_LEG_M / SYNTH_SPEED_MPS;
        let loop_ms = (4.0 * leg_s * 1000.0) as u64;
        let t = (elapsed_ms % loop_ms) as f32 / 1000.0;
        let leg = (t / leg_s) as u32;
        let d = (t - leg as f32 * leg_s) * SYNTH_SPEED_MPS; // metres into this leg
        let (east, north, course) = match leg {
            0 => (d, 0.0, 90.0),                        // →E along the south edge
            1 => (SYNTH_LEG_M, d, 0.0),                 // →N up the east edge
            2 => (SYNTH_LEG_M - d, SYNTH_LEG_M, 270.0), // →W along the north edge
            _ => (0.0, SYNTH_LEG_M - d, 180.0),         // →S down the west edge
        };
        let east = east - SYNTH_LEG_M / 2.0; // centre the square on the centre point
        let north = north - SYNTH_LEG_M / 2.0;
        Some(Fix {
            lon: self.center_lon + (east * self.udeg_per_m_east) as i32,
            lat: self.center_lat + (north * UDEG_PER_M) as i32,
            course: Some(course),
            speed_mps: Some(SYNTH_SPEED_MPS),
        })
    }
}

/// ILI9341 power-on / RGB-interface init, transcribed verbatim from ST's
/// `32f429idiscovery-bsp` `ili9341_Init`: `(command, data bytes, delay-ms-after)`.
/// The load-bearing entries for LTDC operation are 0xB0=0xC2 (RGB interface control)
/// and the second 0xB6 / 0xF6 writes that select the RGB (DPI) data path.
const ILI9341_INIT: &[(u8, &[u8], u16)] = &[
    (0xCA, &[0xC3, 0x08, 0x50], 0),
    (0xCF, &[0x00, 0xC1, 0x30], 0),
    (0xED, &[0x64, 0x03, 0x12, 0x81], 0),
    (0xE8, &[0x85, 0x00, 0x78], 0),
    (0xCB, &[0x39, 0x2C, 0x00, 0x34, 0x02], 0),
    (0xF7, &[0x20], 0),
    (0xEA, &[0x00, 0x00], 0),
    (0xB1, &[0x00, 0x1B], 0),
    (0xB6, &[0x0A, 0xA2], 0),
    (0xC0, &[0x10], 0),
    (0xC1, &[0x10], 0),
    (0xC5, &[0x45, 0x15], 0),
    (0xC7, &[0x90], 0),
    (0x36, &[0xC8], 0), // MADCTL: MY|MX|BGR (ST's orientation; BGR colour order)
    (0xF2, &[0x00], 0),
    (0xB0, &[0xC2], 0), // RGB interface signal control -> RGB/DPI mode (bypass, like ST)
    (0xB6, &[0x0A, 0xA7, 0x27, 0x04], 0),
    (0x2A, &[0x00, 0x00, 0x00, 0xEF], 0), // column 0..239
    (0x2B, &[0x00, 0x00, 0x01, 0x3F], 0), // page 0..319
    (0xF6, &[0x01, 0x00, 0x06], 0),
    (0x2C, &[], 200), // memory write, then settle
    (0x26, &[0x01], 0),
    (0xE0, &[0x0F, 0x29, 0x24, 0x0C, 0x0E, 0x09, 0x4E, 0x78, 0x3C, 0x09, 0x13, 0x05, 0x17, 0x11, 0x00], 0),
    (0xE1, &[0x00, 0x16, 0x1B, 0x04, 0x11, 0x07, 0x31, 0x33, 0x42, 0x05, 0x0C, 0x0A, 0x28, 0x2F, 0x0F], 0),
    (0x11, &[], 200), // sleep out, then settle (datasheet >= 120 ms before display on)
    (0x29, &[], 0),   // display on
    (0x2C, &[], 0),   // memory write start
];

/// Configure one GPIO as an LTDC alternate-function output. Used directly (instead of
/// `Ltdc::new_with_pins`) because the DISC1 wires only RGB666 — the low colour bits
/// have no pin, so the typed 24-pin constructor doesn't fit. The returned `Flex` must
/// be kept alive for the AF config to persist.
fn af_pin(pin: Peri<'static, impl Pin>, af: u8) -> Flex<'static> {
    let mut f = Flex::new(pin);
    f.set_as_af_unchecked(af, AfType::output(OutputType::PushPull, Speed::VeryHigh));
    f
}

/// Idle forever after an unrecoverable bring-up failure. Breaks to the debugger **only when
/// one is attached** (so `probe-rs run` regains control and releases the ST-LINK), then
/// low-power idles. A standalone boot (plain NRST / battery, no debugger) must NOT execute a
/// bare `bkpt` — with `C_DEBUGEN` clear it escalates to a HardFault — so it just `wfi`s.
fn halt() -> ! {
    if cortex_m::peripheral::DCB::is_debugger_attached() {
        cortex_m::asm::bkpt();
    }
    loop {
        cortex_m::asm::wfi();
    }
}

/// Point an LTDC `layer` at the framebuffer at `addr`, reloading at the next vertical
/// blank — the double-buffer flip. Both planes use it: the map (Layer 1) and the overlay
/// (Layer 2) are each double-buffered, so neither ever draws into the buffer being scanned
/// out. Unlike the immediate reload used at init, the **vblank** reload switches buffers
/// between frames, so there's no tear; the bounded poll on the hardware-cleared `VBR` bit
/// then waits for the switch to actually land (so the old front is free to reuse) without
/// ever spinning forever — the same "no blocking wait that can hang probe-rs" rule init
/// follows by avoiding the reload interrupt. A frame is ~15 ms at the 6 MHz DOTCLK, so the
/// reload lands well within the 50 ms cap in normal operation.
///
/// The single shared `SRCR.VBR` reloads *both* layers' shadow registers at the vblank, but a
/// flip only changes the named layer's `CFBAR` shadow; the other layer's shadow is unchanged,
/// so re-applying it is a no-op. Flipping one layer therefore never disturbs the other.
///
/// In the two-plane build (issue #48) the two layers flip from **different executors** — the
/// map (Layer 1) from thread mode, the overlay (Layer 2) from the high-priority interrupt
/// executor — so the `CFBAR` write + the shared `SRCR.VBR` read-modify-write are done inside a
/// short critical section. The per-layer `CFBAR` is independent, but `SRCR` also holds the
/// `IMR` field, so an un-guarded RMW could tear across a preemption. The **poll** stays outside
/// the critical section so it never blocks the other plane: if both planes flip near the same
/// vblank, the one pending reload simply applies both layers' shadows and both polls observe it.
///
/// Returns `true` if the reload **landed** (the hardware cleared `VBR` within the cap),
/// `false` on timeout. The caller swaps front/back only on `true`: on a timeout the LTDC is
/// still scanning the old front, so swapping roles would point the next render at the buffer
/// being scanned out — a torn frame. Keeping the buffers on timeout instead lets the next
/// redraw re-render into the same back and retry the flip. (Rare — the 50 ms cap is ~3 frames.)
///
/// This is the **synchronous** flip the map plane (thread mode) uses — its busy-poll blocks
/// only itself (the lowest priority). The overlay task on the high-priority interrupt executor
/// must NOT busy-poll (that would hold the ISR and starve the map render it just preempted), so
/// it uses [`request_flip`] + [`flip_landed`] and `await`s the vblank with `Timer` yields.
#[cfg(not(feature = "glass-demo"))]
#[must_use]
fn flip_to(layer: LtdcLayer, addr: usize) -> bool {
    request_flip(layer, addr);
    let t0 = Instant::now();
    while !flip_landed() {
        if t0.saturating_elapsed().as_millis() > 50 {
            return false;
        }
    }
    true
}

/// Arm a vblank framebuffer flip for `layer` to `addr`: point its `CFBAR` at the new buffer and
/// request the shared `SRCR` vblank reload. Guarded by a short critical section so the per-layer
/// `CFBAR` write + the shared `SRCR.VBR` read-modify-write can't tear against a flip on the other
/// executor (the two planes flip from different priorities — see [`flip_to`]). The reload itself
/// lands at the next vblank; the caller waits for it via [`flip_landed`].
#[cfg(not(feature = "glass-demo"))]
fn request_flip(layer: LtdcLayer, addr: usize) {
    use stm32_metapac::ltdc::vals::Vbr;
    use stm32_metapac::LTDC;
    critical_section::with(|_| {
        LTDC.layer(layer as usize).cfbar().modify(|w| w.set_cfbadd(addr as u32));
        LTDC.srcr().modify(|w| w.set_vbr(Vbr::RELOAD));
    });
}

/// Whether the pending vblank reload armed by [`request_flip`] has landed (the hardware cleared
/// `SRCR.VBR`). Read-only, so it is safe to poll concurrently from both planes: if both armed a
/// reload near the same vblank, the one reload applies both layers' shadow registers and both
/// observe `VBR` clear.
#[cfg(not(feature = "glass-demo"))]
fn flip_landed() -> bool {
    use stm32_metapac::ltdc::vals::Vbr;
    use stm32_metapac::LTDC;
    LTDC.srcr().read().vbr() != Vbr::RELOAD
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // 168 MHz core from HSI (16/8=2 MHz -> x168 -> /2). VCO = 336 MHz so PLL_Q = 336/7 = 48 MHz
    // feeds the USB OTG full-speed clock (issue #38) — F429 has no CK48 mux, so the 48 MHz must
    // come from PLL_Q, which forces the classic F4 168/48 pair (180 MHz can't yield an integer
    // /Q = 48). The LTDC pixel clock is independent: a PLLSAI leg gives VCO = 2 x 96 = 192 MHz,
    // /R(4) = 48 MHz, /PLLSAIDIVR(8) = 6 MHz DOTCLK (matching ST's BSP). embassy's ltdc driver
    // hard-codes PLLSAIDIVR=2, so the DIV8 is forced back on via the PAC right after `Ltdc::new()`.
    let p = {
        let mut config = embassy_stm32::Config::default();
        use embassy_stm32::rcc::*;
        config.rcc.hsi = true;
        config.rcc.pll_src = PllSource::HSI;
        config.rcc.pll = Some(Pll {
            prediv: PllPreDiv::DIV8,
            mul: PllMul::MUL168,       // VCO = 2 x 168 = 336 MHz
            divp: Some(PllPDiv::DIV2), // SYSCLK = 336 / 2 = 168 MHz
            divq: Some(PllQDiv::DIV7), // PLL48CLK = 336 / 7 = 48 MHz (USB OTG FS clock)
            divr: None,
        });
        config.rcc.pllsai = Some(Pll {
            prediv: PllPreDiv::DIV8, // shared M with the main PLL -> 2 MHz input
            mul: PllMul::MUL96,      // VCO = 2 x 96 = 192 MHz (same VCO as ST's BSP)
            divp: None,
            divq: None,
            divr: Some(PllRDiv::DIV4), // PLLSAI_R = 192 / 4 = 48 MHz
        });
        config.rcc.sys = Sysclk::PLL1_P;
        config.rcc.ahb_pre = AHBPrescaler::DIV1; // HCLK 168 MHz
        config.rcc.apb1_pre = APBPrescaler::DIV4; // PCLK1 42 MHz (<= 45 MHz)
        config.rcc.apb2_pre = APBPrescaler::DIV2; // PCLK2 84 MHz (<= 90 MHz)
        embassy_stm32::init(config)
    };

    defmt::info!("obc-fw-stm32f429: phase C (ILI9341 + LTDC), 168 MHz core / 6 MHz pixel clock");

    // Bring up the USB-CDC fake-sensor link first (issue #38) so it enumerates while the display
    // and SD card come up; the parsed fixes land in obc-platform's signals, ready for the app's
    // sensor poll below. Compiled out (and `spawner` unused) when `debug-usb` is off — then the
    // app falls back to the SynthLocation stand-in.
    #[cfg(feature = "debug-usb")]
    {
        use static_cell::StaticCell;
        // 'static buffers the USB device + CDC class borrow for the whole run.
        static EP_OUT: StaticCell<[u8; 256]> = StaticCell::new();
        static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESC: StaticCell<[u8; 32]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
        static STATE: StaticCell<State<'static>> = StaticCell::new();

        let mut otg_cfg = usb::Config::default();
        // The DISC1 USER port doesn't wire VBUS sense to the MCU; assume the cable is present.
        otg_cfg.vbus_detection = false;
        let driver = usb::Driver::new_fs(p.USB_OTG_HS, UsbIrqs, p.PB15, p.PB14, EP_OUT.init([0; 256]), otg_cfg);

        // Device descriptor: a generic CDC-ACM serial device (pid.codes test VID/PID).
        let mut dev_cfg = embassy_usb::Config::new(0x16c0, 0x27dd);
        dev_cfg.manufacturer = Some("OpenBikeComputer");
        dev_cfg.product = Some("OBC debug sensors");
        dev_cfg.serial_number = Some("obc-f429");
        dev_cfg.max_power = 100;
        dev_cfg.max_packet_size_0 = 64;

        let mut builder = embassy_usb::Builder::new(
            driver,
            dev_cfg,
            CONFIG_DESC.init([0; 256]),
            BOS_DESC.init([0; 32]),
            &mut [], // no Microsoft OS descriptors
            CONTROL_BUF.init([0; 128]),
        );
        let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
        let (tx, rx) = class.split();
        spawner.spawn(defmt::unwrap!(usb_device_task(builder.build())));
        spawner.spawn(defmt::unwrap!(usb_rx_task(rx)));
        spawner.spawn(defmt::unwrap!(usb_tx_task(tx)));
        defmt::info!("USB-CDC debug sensors up on OTG_HS (USER port, PB14/PB15)");
    }
    #[cfg(not(feature = "debug-usb"))]
    let _ = spawner;

    let mut green = Output::new(p.PG13, Level::High, Speed::Low);
    let _red = Output::new(p.PG14, Level::Low, Speed::Low);

    // --- SDRAM (framebuffer storage) ---
    let mut sdram = Fmc::sdram_a12bits_d16bits_4banks_bank2(
        p.FMC,
        p.PF0,
        p.PF1,
        p.PF2,
        p.PF3,
        p.PF4,
        p.PF5,
        p.PF12,
        p.PF13,
        p.PF14,
        p.PF15,
        p.PG0,
        p.PG1,
        p.PG4,
        p.PG5,
        p.PD14,
        p.PD15,
        p.PD0,
        p.PD1,
        p.PE7,
        p.PE8,
        p.PE9,
        p.PE10,
        p.PE11,
        p.PE12,
        p.PE13,
        p.PE14,
        p.PE15,
        p.PD8,
        p.PD9,
        p.PD10,
        p.PE0,
        p.PE1,
        p.PB5,
        p.PG8,
        p.PG15,
        p.PB6,
        p.PF11,
        p.PC0,
        stm32_fmc::devices::is42s16400j_7::Is42s16400j {},
    );
    let ram_ptr: *mut u32 = sdram.init(&mut Delay);
    defmt::info!("SDRAM base {=u32:#010x}", ram_ptr as u32);

    // --- SDRAM framebuffer(s). Cleared to black up front so the panel shows black, not
    // SDRAM garbage, in the window between LTDC turn-on and the first rendered frame.
    // The LTDC starts scanning FB_ADDR (the front buffer); the app path adds a second
    // buffer just past it and double-buffers (see the render loop), so each buffer is
    // wrapped in its own short-lived slice instead of one long-lived `&mut` — two live
    // `&mut` over the same SDRAM would alias.
    // SAFETY: the FMC maps a contiguous FB_PIXELS×u16 region at FB_ADDR (verified in
    // phase B); the borrow ends at the statement, and the LTDC only *reads* it by DMA. ---
    unsafe { core::slice::from_raw_parts_mut(FB_ADDR as *mut u16, FB_PIXELS) }.fill(0x0000);
    defmt::info!("framebuffer ready: {=usize}x{=usize} RGB565 in SDRAM, cleared", W, H);

    // --- LTDC: drive the panel's RGB lines, scanning the SDRAM framebuffer ---
    // Brought up BEFORE the ILI9341 so the sync/DE/DOTCLK are already running when the
    // panel enters RGB mode (ST's BSP does the same order); the panel locks its RGB
    // interface onto the live sync instead of free-running.
    // 240x320, timings from ST's BSP: HSYNC 10 / HBP 20 / HFP 10, VSYNC 2 / VBP 2 / VFP 4.
    let ltdc_config = LtdcConfiguration {
        active_width: W as u16,
        active_height: H as u16,
        h_back_porch: 20,
        h_front_porch: 10,
        v_back_porch: 2,
        v_front_porch: 4,
        h_sync: 10,
        v_sync: 2,
        h_sync_polarity: PolarityActive::ActiveLow,
        v_sync_polarity: PolarityActive::ActiveLow,
        data_enable_polarity: PolarityActive::ActiveLow,
        pixel_clock_polarity: PolarityEdge::RisingEdge,
    };

    let mut ltdc = Ltdc::new(p.LTDC);
    // `Ltdc::new` forces PLLSAIDIVR = 2 (giving 48/2 = 24 MHz); override it to 8 so the
    // DOTCLK is 48/8 = 6 MHz, exactly matching ST's BSP (the ILI9341 RGB interface mis-
    // samples above ~6 MHz, which sheared the image at our earlier 8 MHz).
    stm32_metapac::RCC.dckcfgr().modify(|w| w.set_pllsaidivr(stm32_metapac::rcc::vals::Pllsaidivr::DIV8));
    // Drive only the 18 wired RGB666 bits + 4 sync/clk/de lines (AF14, except the
    // four AF9 pins). Kept alive in `_ltdc_pins` so the AF config persists.
    let _ltdc_pins = [
        af_pin(p.PC6, 14),
        af_pin(p.PA4, 14),
        af_pin(p.PG7, 14),
        af_pin(p.PF10, 14), // HSYNC VSYNC CLK DE
        af_pin(p.PC10, 14),
        af_pin(p.PA11, 14),
        af_pin(p.PA12, 14),
        af_pin(p.PG6, 14), // R2 R4 R5 R7
        af_pin(p.PA6, 14),
        af_pin(p.PB10, 14),
        af_pin(p.PB11, 14),
        af_pin(p.PC7, 14),
        af_pin(p.PD3, 14), // G2 G4 G5 G6 G7
        af_pin(p.PD6, 14),
        af_pin(p.PG11, 14),
        af_pin(p.PA3, 14),
        af_pin(p.PB8, 14),
        af_pin(p.PB9, 14), // B2 B3 B5 B6 B7
        af_pin(p.PB0, 9),
        af_pin(p.PB1, 9),
        af_pin(p.PG10, 9),
        af_pin(p.PG12, 9), // R3 R6 G3 B4
    ];
    ltdc.init(&ltdc_config);

    let layer_config = LtdcLayerConfig {
        pixel_format: PixelFormat::RGB565,
        layer: LtdcLayer::Layer1,
        window_x0: 0,
        window_x1: W as u16,
        window_y0: 0,
        window_y1: H as u16,
    };
    ltdc.init_layer(&layer_config, None);
    // embassy's init_layer programs CFBLL = active*bpp + 7, but RM0090 specifies
    // active*bpp + 3; that extra 4-byte line over-read carries into the next line and
    // is the per-line horizontal shear. Reprogram CFBLR with the spec value (and set
    // CFBP explicitly — the metapac field write zeroes the other field otherwise).
    // RGB565: pitch 240*2 = 480, line length 480 + 3 = 483.
    stm32_metapac::LTDC.layer(0).cfblr().modify(|w| {
        w.set_cfbp(480);
        w.set_cfbll(483);
    });

    // Point layer 1 at the SDRAM framebuffer and request a vblank reload. Done via the
    // PAC rather than `Ltdc::set_buffer().await` so bring-up doesn't depend on the LTDC
    // reload interrupt (a never-firing wait would hang probe-rs -> stuck ST-LINK).
    {
        use stm32_metapac::ltdc::vals::Imr;
        use stm32_metapac::LTDC;
        LTDC.layer(LtdcLayer::Layer1 as usize).cfbar().modify(|w| w.set_cfbadd(FB_ADDR as u32));
        LTDC.srcr().write(|w| w.set_imr(Imr::RELOAD)); // immediate reload (no vblank/interrupt wait)
    }

    // --- ILI9341 init over SPI5, now that the LTDC sync is live (panel into RGB/DPI
    // mode + display on; it locks onto the running HSYNC/VSYNC/DE) ---
    let mut spi_cfg = embassy_stm32::spi::Config::default();
    spi_cfg.frequency = Hertz(5_000_000); // ST uses 5.6 MHz; mode 0 is the default
    let mut spi = Spi::new_blocking_txonly(p.SPI5, p.PF7, p.PF9, spi_cfg);
    let mut cs = Output::new(p.PC2, Level::High, Speed::VeryHigh);
    let mut dcx = Output::new(p.PD13, Level::High, Speed::VeryHigh);

    Timer::after_millis(20).await; // panel power-up after NRST release
    for &(cmd, data, delay_ms) in ILI9341_INIT {
        cs.set_low();
        dcx.set_low(); // command
        let _ = spi.blocking_write(&[cmd]);
        if !data.is_empty() {
            dcx.set_high(); // data
            let _ = spi.blocking_write(data);
        }
        cs.set_high();
        if delay_ms > 0 {
            Timer::after_millis(delay_ms as u64).await;
        }
    }
    defmt::info!("ILI9341 init done");

    green.set_high();
    defmt::info!("display up: LTDC scanning SDRAM framebuffer");

    // --- glass-demo (issue #33 leftover): font ladder + palette on glass, then halt.
    // The device analog of the simulator's `--text-demo`; verifies the text raster +
    // RGB565 colour path in isolation before the whole app is pointed at the panel. ---
    #[cfg(feature = "glass-demo")]
    {
        // Single-buffer demo (a static screen, then halt — no flicker to fix).
        // SAFETY: the LTDC scans FB_ADDR and we only write it; sole borrow.
        let fb_buf = unsafe { core::slice::from_raw_parts_mut(FB_ADDR as *mut u16, FB_PIXELS) };
        let mut fb = Framebuffer565::new(fb_buf, W as u32, H as u32);
        let _ = demo::font_palette_demo(&mut fb);
        defmt::info!("glass-demo: font + palette rendered; halting");
        halt()
    }

    // --- obc-app on glass, button-driven, now reading real data off SD (issues #34/#35/#36):
    // boot Home/Idle, list the card's routes, ride the chosen route on the card's map, and save
    // the ride back as a `.gpx`. A missing/bad card degrades to the baked tile + a stub route. ---
    #[cfg(not(feature = "glass-demo"))]
    {
        // microSD over SPI4 (CS = PD7). Init the bus slow (SD spec ≤400 kHz); `sd::init`
        // re-clocks it after the card is up. A `None` here is a missing/bad card — handled by
        // falling back below, never a panic (acceptance criterion).
        let mut sd_cfg = embassy_stm32::spi::Config::default();
        sd_cfg.frequency = Hertz(400_000);
        sd_cfg.miso_pull = Pull::Up; // hold DO high when the card isn't driving the line
        let sd_spi = Spi::new_blocking(p.SPI4, p.PE2, p.PE6, p.PE5, sd_cfg);
        let sd_cs = Output::new(p.PD7, Level::High, Speed::VeryHigh);
        let mut storage = sd::init(sd_spi, sd_cs);

        // --- LTDC Layer 2: the transparent UI overlay plane (issue #46) ---
        // The map renders to the double-buffered Layer 1 (below); the hold-bulge / confirm ring
        // renders to this second, per-pixel-alpha-blended layer (above), so it composites over the
        // map in hardware and repaints without ever touching — or re-rendering — the map buffer.
        // embassy's `init_layer` already programs the blend we want: BF1/BF2 = pixel-alpha
        // (BC = α·overlay + (1−α)·map), constant alpha 0xFF, and a transparent-black default colour
        // (DCCR = 0) for the pixels outside the (here full-screen) window. ARGB4444 is plenty for the
        // opaque near-black bulge; alpha 0 = transparent lets the map show through everywhere the
        // overlay leaves blank. Clear the front buffer fully transparent first so Layer 2 shows
        // nothing (the map shows through) until the first hold; the back is cleared in the loop
        // setup below.
        // SAFETY: OVERLAY_ADDR..+FB_BYTES is a distinct SDRAM region past both map framebuffers and
        // before the back overlay buffer / App slot; sole owner, the LTDC only DMA-reads it.
        unsafe { core::slice::from_raw_parts_mut(OVERLAY_ADDR as *mut u16, FB_PIXELS) }.fill(0x0000);
        ltdc.init_layer(
            &LtdcLayerConfig {
                pixel_format: PixelFormat::ARGB4444,
                layer: LtdcLayer::Layer2,
                window_x0: 0,
                window_x1: W as u16,
                window_y0: 0,
                window_y1: H as u16,
            },
            None,
        );
        // Mirror the Layer-1 CFBLR fix (see the Layer-1 setup): embassy programs CFBLL = active*bpp
        // + 7, but RM0090 specifies + 3; the extra 4-byte over-read shears each line. ARGB4444 is
        // 2 B/px like RGB565, so the values are identical — pitch 240*2 = 480, line length 480 + 3.
        stm32_metapac::LTDC.layer(LtdcLayer::Layer2 as usize).cfblr().modify(|w| {
            w.set_cfbp(480);
            w.set_cfbll(483);
        });
        // Point Layer 2 at its front framebuffer and reload immediately (no vblank wait, same as
        // Layer 1 at init). Thereafter the overlay is double-buffered: each frame is rendered into
        // the back buffer and `flip_to`d at the vblank (see the loop), so this CFBAR is updated by
        // those flips, not held fixed.
        {
            use stm32_metapac::ltdc::vals::Imr;
            use stm32_metapac::LTDC;
            LTDC.layer(LtdcLayer::Layer2 as usize).cfbar().modify(|w| w.set_cfbadd(OVERLAY_ADDR as u32));
            LTDC.srcr().write(|w| w.set_imr(Imr::RELOAD));
        }

        // SDRAM layout: past the four framebuffers (the double-buffered map plane + the
        // double-buffered Layer 2 overlay, see [`OVERLAY_ADDR`]) sits the App, then the streamed
        // map's `MapCache` (issue #37 — the geometry/index cache, placed once and reused across
        // redraws). Both live in SDRAM, off the 192 KB main stack. No resident full-tile buffer
        // any more: the map streams from the open `.obcm` on the card.
        let app_align = core::mem::align_of::<App>();
        let app_addr = (SDRAM_ADDR + 4 * FB_BYTES + app_align - 1) & !(app_align - 1);
        let cache_align = core::mem::align_of::<MapCache>();
        let cache_addr = (app_addr + core::mem::size_of::<App>() + cache_align - 1) & !(cache_align - 1);

        // The streamed-map cache, placed once in SDRAM and reused every redraw. `MapCache::new`
        // is `MaybeUninit::zeroed()` (every field is valid all-zero), so `ptr::write` lowers to a
        // `memset` of the slot — its ~130 KB never lands on the stack, with no reliance on RVO.
        // (The App below is placed the same way in spirit, field-by-field via `App::init_idle`.)
        // SAFETY: [cache_addr, cache_addr + size_of::<MapCache>()) is a distinct SDRAM region just
        // past the App slot and clear of the framebuffers; this is its sole owner for the run.
        let cache_ptr = cache_addr as *mut MapCache;
        unsafe { cache_ptr.write(MapCache::new()) };
        let map_cache: &MapCache = unsafe { &*cache_ptr };

        // Map: open the card's `.obcm` and **stream** it (issue #37 — no resident full-tile
        // buffer), else fall back to the baked-in tile (flash-resident), else halt. `map_streaming`
        // selects the per-frame source in the render loop below.
        let map_streaming = storage.as_mut().and_then(|s| s.open_map()).is_some();
        if map_streaming {
            defmt::info!("map: streaming from SD card");
        } else {
            #[cfg(feature = "baked-tile")]
            defmt::info!("map: {=usize} B baked-in tile (no SD card map)", TILE.len());
            #[cfg(not(feature = "baked-tile"))]
            {
                defmt::error!("no SD card map and baked-tile is disabled — halting");
                halt()
            }
        }

        // One-shot parse to validate the map + read its bbox for the idle camera centre. The
        // throwaway source/reader is dropped here; the open file handle stays for the loop's
        // per-frame readers. (The shared cache is empty at startup, so this warms nothing.)
        let (cam_lon, cam_lat) = {
            #[cfg(feature = "baked-tile")]
            let baked_src = SliceSource(TILE);
            let init_sd_src = storage.as_ref().and_then(|s| s.map_source());
            let init_src: &dyn ByteSource = match &init_sd_src {
                Some(s) => s,
                #[cfg(feature = "baked-tile")]
                None => &baked_src,
                #[cfg(not(feature = "baked-tile"))]
                None => halt(),
            };
            match Reader::new(init_src, map_cache) {
                Ok(reader) => (
                    ((reader.bbox.min_lon as i64 + reader.bbox.max_lon as i64) / 2) as i32,
                    ((reader.bbox.min_lat as i64 + reader.bbox.max_lat as i64) / 2) as i32,
                ),
                Err(_) => {
                    defmt::error!("map is not valid OBCM — halting");
                    halt()
                }
            }
        };

        // Native RGB565 panel → the color_fn is the identity (device-64 quantization is a
        // host/simulator concern; see obc-platform::framebuffer).
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

        // Build the App *in place* in SDRAM at the reserved slot. `App::init_idle` writes each
        // field straight into the slot (the ~200 KB renderer is zeroed in place), so the App is
        // never formed by value — no 200 KB stack temporary, and no reliance on the optimizer's
        // RVO to keep one off the 192 KB stack (issue #67).
        let app_ptr = app_addr as *mut App;
        defmt::info!(
            "App in SDRAM @ {=u32:#010x} ({=usize} B); map cache @ {=u32:#010x} ({=usize} B)",
            app_addr as u32,
            core::mem::size_of::<App>(),
            cache_addr as u32,
            core::mem::size_of::<MapCache>()
        );
        // Power-on screen: Home / Idle — the user drives navigation from here.
        // SAFETY: app_ptr is a valid, aligned, exclusively-owned SDRAM slot; init_idle fully
        // initializes it before the &mut below reads it.
        unsafe {
            App::init_idle(app_ptr, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
        }
        let app = unsafe { &mut *app_ptr };

        // Routes: the card's `/routes` catalog feeds the Route menu; with no card a single stub
        // route keeps the Map reachable (the pre-#36 behaviour).
        match storage.as_mut() {
            Some(s) => {
                let catalog = s.scan_routes();
                app.set_routes(&catalog);
            }
            None => {
                let mut name = heapless::String::new();
                let _ = name.push_str("Teningen");
                app.set_routes(&[RouteSummary {
                    name,
                    distance_km: 0,
                    climb_m: 0,
                    bbox: BBox { min_lon: cam_lon, min_lat: cam_lat, max_lon: cam_lon, max_lat: cam_lat },
                    start_lon: cam_lon,
                    start_lat: cam_lat,
                }]);
            }
        }

        // The wall clock the loop's `now` reads (and, without `debug-usb`, the SynthLocation
        // stand-in). Kept regardless of which sensor source is wired below.
        let start = Instant::now();
        // Sensor sources. Default (`debug-usb`): the host-streamed GPS / altimeter / compass
        // (issue #38), parsed by the USB tasks into obc-platform's signals — these ZST handles
        // just `try_take` from them. Fallback (`debug-usb` off): the SynthLocation square-loop,
        // with no altimeter/compass. Same `Sensors` either way, so the app can't tell.
        #[cfg(feature = "debug-usb")]
        let (mut debug_loc, mut debug_alt, mut debug_compass) = (
            obc_platform::debug_usb::DebugLocation,
            obc_platform::debug_usb::DebugAltimeter,
            obc_platform::debug_usb::DebugCompass,
        );
        #[cfg(not(feature = "debug-usb"))]
        let mut synth = SynthLocation::new(cam_lon, cam_lat, start);

        // Double buffering (the map plane / Layer 1): the LTDC scans the *front* while the app
        // renders the *back*, then we flip at the next vblank (tear-free) and swap. FB_ADDR is the
        // initial front; the back sits one FB_BYTES past it. Owned by the map plane (this loop) in
        // both builds. SAFETY: the back is never the buffer the LTDC is scanning.
        let mut front_addr = FB_ADDR;
        let mut back_addr = FB_ADDR + FB_BYTES;
        unsafe { core::slice::from_raw_parts_mut(back_addr as *mut u16, FB_PIXELS) }.fill(0x0000);

        // --- input + overlay plane wiring (issue #48) ---
        // The pushbuttons (issue #35) + the Layer-2 overlay double-buffer belong to the *input
        // plane*. In the default two-plane build that plane runs on the high-priority interrupt
        // executor, so its state moves into the spawned task and the map loop only drains the
        // recognised gestures off the channel. The `single-executor` fallback keeps it all inline.
        //
        // Layer 2's *back* buffer never needs pre-clearing: the overlay render clears it to
        // transparent before every flip, and the *front* (OVERLAY_ADDR) was cleared at Layer-2
        // init, so the panel shows transparent (the map) until the first hold either way.

        // Two-plane (default): stand up the high-priority executor, hand it the buttons, a
        // standalone `InputPlane`, the Layer-2 overlay double-buffer and the gesture channel
        // sender; the map loop keeps the receiver. UART5 is unused on this board, so its interrupt
        // vector is the executor's pend line; P6 is above thread mode (so it preempts the map
        // render) and below the embassy-time driver (so its `Timer`s still wake mid-render).
        #[cfg(not(feature = "single-executor"))]
        let gestures: Receiver<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = {
            let buttons = ButtonInput::new(
                Input::new(p.PD4, Pull::Up), // PREV   → Turn(-1)
                Input::new(p.PE3, Pull::Up), // NEXT   → Turn(+1)
                Input::new(p.PE4, Pull::Up), // SELECT → encoder press / hold
                Input::new(p.PD5, Pull::Up), // BACK   → back / back-hold
            );
            interrupt::UART5.set_priority(Priority::P6);
            let input_spawner = EXECUTOR_INPUT.start(interrupt::UART5);
            input_spawner.spawn(defmt::unwrap!(input_overlay_task(
                buttons,
                InputPlane::new(),
                GESTURES.sender(),
                OVERLAY_ADDR,
                OVERLAY_ADDR + FB_BYTES,
                start,
            )));
            defmt::info!(
                "input plane: high-priority interrupt executor (UART5 @ P6), preempting the map render; map plane: thread mode; SD card {=bool}",
                storage.is_some()
            );
            GESTURES.receiver()
        };

        // Single-executor fallback: buttons + the overlay double-buffer stay in this loop, driven
        // inline through `App::handle_input` + `render_overlay` — proving the `InputPlane` /
        // `apply_gesture` seam composes back into one cooperative loop.
        #[cfg(feature = "single-executor")]
        let (mut buttons, mut overlay_front, mut overlay_back) = {
            let buttons = ButtonInput::new(
                Input::new(p.PD4, Pull::Up), // PREV   → Turn(-1)
                Input::new(p.PE3, Pull::Up), // NEXT   → Turn(+1)
                Input::new(p.PE4, Pull::Up), // SELECT → encoder press / hold
                Input::new(p.PD5, Pull::Up), // BACK   → back / back-hold
            );
            defmt::info!(
                "input live (single-executor): PD4 PREV / PE3 NEXT / PE4 SELECT / PD5 BACK; map + overlay both double-buffered; SD card {=bool}",
                storage.is_some()
            );
            (buttons, OVERLAY_ADDR, OVERLAY_ADDR + FB_BYTES)
        };

        // Render-on-demand loop, split across the two LTDC layers (issue #46) and driven by the
        // app's **dirty signal** (issue #47): the shared `App` accumulates which planes changed as
        // it ticks/handles input, and `App::take_dirty` reports it once per frame. The **map**
        // (Layer 1) re-renders only when `dirty.map` — a gesture, a camera-moving GPS fix, a route
        // load, or a screen's timed content (the Statistics spring-back) — so a genuinely static
        // Map performs **zero** map renders (no more blind 1 s heartbeat). The hold-bulge overlay
        // (Layer 2) repaints independently of the map: on the two-plane build that happens on the
        // high-priority plane above; on the single-executor build it repaints here on `dirty.overlay`.
        // The previously-active route, to re-centre the SynthLocation loop when it changes (the
        // USB feed needs no re-centre — it streams absolute positions, so this is `debug-usb`-off only).
        #[cfg(not(feature = "debug-usb"))]
        let mut prev_route: Option<usize> = None;
        // Telemetry throttle + the last map frame's render stats, published host-ward (issue #38).
        #[cfg(feature = "debug-usb")]
        let mut last_telem_ms: u32 = 0;
        #[cfg(feature = "debug-usb")]
        let mut last_telem = obc_platform::debug_usb::Telemetry::default();
        // The active route's parsed chunk index, cached across frames (issue #44). `index_route`
        // is the route it belongs to; a mismatch triggers one rebuild off the open file, so a
        // per-frame redraw streams geometry without re-walking the index off the SD card.
        let mut route_index: Option<RouteIndex> = None;
        let mut index_route: Option<usize> = None;
        // Latched when a frame's `dirty.map` edge can't be serviced because `Reader::new` failed on
        // a transient SD glitch (issue #66). The map dirty signal is an *accumulated edge* (unlike
        // the overlay's, which `take_dirty` recomputes from live state each frame), so once the
        // drain consumes it and the redraw is skipped the demand is gone — the map would stay stale
        // until some unrelated mutation re-dirties it. We OR this back into next frame's `dirty.map`
        // so the redraw retries until the reader builds.
        let mut pending_map_redraw = false;
        loop {
            let now = start.saturating_elapsed().as_millis() as u32;

            // --- input (map-plane side) ---
            // Two-plane: the high-priority plane has already recognised this frame's gestures and
            // animated the overlay; here we just drain them in order and apply each to the screen
            // stack, then advance the visible screens' timed content (the Statistics spring-back).
            // So the screen transition lands a frame after the overlay already confirmed the press.
            #[cfg(not(feature = "single-executor"))]
            {
                while let Ok(g) = gestures.try_receive() {
                    app.apply_gesture(g);
                }
                app.advance_animations(InputClock(now));
            }
            // Single-executor: recognise + apply + animate inline, the cooperative path. Physical
            // buttons + (with `debug-usb`) the USB-injected events, chained into one pass.
            #[cfg(feature = "single-executor")]
            {
                buttons.update(now);
                #[cfg(feature = "debug-usb")]
                let mut usb_in = obc_platform::debug_usb::DebugInput;
                #[cfg(not(feature = "debug-usb"))]
                let mut usb_in = NullInput;
                let mut input = ChainedInput { a: &mut buttons, b: &mut usb_in };
                app.handle_input(InputClock(now), &mut input);
            }

            let active = app.activity.active_route;
            // Re-centre the synthetic GPS onto a freshly-loaded route's start so Follow doesn't
            // yank the camera off it. Only the `debug-usb`-off fallback: the USB feed streams
            // absolute positions, so it needs no re-centre.
            #[cfg(not(feature = "debug-usb"))]
            if active != prev_route {
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    synth.recenter(r.start_lon, r.start_lat);
                }
                prev_route = active;
            }

            // Reconcile the card to the app's intent: open/close the active route's geometry and
            // the ride log (begin on load, finalise-to-GPX on Finish), reading the save name from
            // the active route. Cheap when nothing changed.
            if let Some(s) = storage.as_mut() {
                let action = app.activity.take_track_action();
                let session = app.activity.session;
                let mut name: heapless::String<64> = heapless::String::new();
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    let _ = name.push_str(&r.name);
                }
                s.reconcile_route(active);
                s.reconcile_track(action, session, &name);
            }

            // Cache the active route's chunk index across frames: rebuild it (the expensive
            // header + full chunk-meta walk off SD) only when the route changes, or retry if a
            // prior build failed on a flaky link. Not gated on rendering — the matcher in `tick`
            // below needs the index on every *fresh fix*, independent of whether the map redraws,
            // so it's ready as soon as the route is loaded (`reconcile_route` above opened the file).
            if index_route != active {
                match active {
                    Some(_) => {
                        match storage.as_ref().and_then(|s| s.build_route_index()) {
                            Some(idx) => {
                                route_index = Some(idx);
                                index_route = active; // cached — no more rebuilds until the route changes
                            }
                            None => {
                                // Transient SD glitch: clear the cache *key* too (not just the
                                // index) so the mismatch persists and every frame retries —
                                // leaving `index_route` stale would suppress the rebuild if the
                                // user swapped away and back to this route. Hide it this frame
                                // rather than the whole ride.
                                route_index = None;
                                index_route = None;
                                defmt::warn!("SD: route index read failed (flaky link?) — retrying next frame");
                            }
                        }
                    }
                    None => {
                        route_index = None;
                        index_route = None;
                    }
                }
            }
            // This frame's reader = the cached index + a fresh geometry source. `RouteReader::new`
            // and `route_source` do no I/O (the source just wraps the open file handle); geometry
            // streams lazily via the source (`decode_chunk`) only where it's actually read — the
            // matcher on a fresh fix, and the renderer on a map-redraw frame. So opening it every
            // frame is cheap, and decouples matching from the render cadence (issue #43/#47).
            let route_src = storage.as_ref().and_then(|s| s.route_source());
            let route = match (route_index.as_ref(), route_src.as_ref()) {
                (Some(idx), Some(src)) => Some(RouteReader::new(idx, src)),
                _ => None,
            };
            // The ride-log sink, by contrast, is built *every* tick — it only wraps the already
            // open log file handle (no I/O), so a fresh fix is written to the `.gpx` the moment it
            // arrives, at the fix rate, independent of the render cadence. Gating it on redraw
            // made *which* fixes reached the log depend on redraw phase rather than time (#43).
            let mut tsink = storage.as_ref().and_then(|s| s.track_sink());
            let track_dyn = tsink.as_mut().map(|t| t as &mut dyn TrackSink);

            // Default: the USB-streamed GPS + altimeter + compass (issue #38). Fallback: the
            // SynthLocation square loop, no altimeter/compass. `track_dyn` is consumed either way.
            #[cfg(feature = "debug-usb")]
            app.tick(
                RideClock(now),
                Sensors {
                    loc: &mut debug_loc,
                    altimeter: Some(&mut debug_alt),
                    compass: Some(&mut debug_compass),
                    track: track_dyn,
                },
                route.as_ref(),
            );
            #[cfg(not(feature = "debug-usb"))]
            app.tick(
                RideClock(now),
                Sensors { loc: &mut synth, altimeter: None, compass: None, track: track_dyn },
                route.as_ref(),
            );

            // Drain the per-frame dirty signal (issue #47) now that input + tick have run: it
            // tells us which planes actually changed, so each render below is on-demand.
            let mut dirty = app.take_dirty();
            // Re-arm a map redraw a previous frame couldn't service (issue #66): fold the latch in
            // and clear it, so a redraw that the skip path below re-latches is retried next frame.
            dirty.map |= pending_map_redraw;
            pending_map_redraw = false;

            if dirty.map {
                let t0 = Instant::now();
                // Render into the back buffer (not the one being scanned out).
                // SAFETY: back_addr is the buffer the LTDC is *not* scanning; the only live
                // `&mut` over it, dropped before the flip.
                let back = unsafe { core::slice::from_raw_parts_mut(back_addr as *mut u16, FB_PIXELS) };
                let mut fb = Framebuffer565::new(back, W as u32, H as u32);

                // This frame's map source: streamed from the open SD `.obcm` (issue #37), or the
                // baked tile (flash-resident). Both are `ByteSource`s; the small `Reader` is
                // rebuilt per redraw against the session-long SDRAM `MapCache`, so a chunk read on
                // a previous frame can still hit this frame (cross-frame reuse).
                let sd_src = if map_streaming { storage.as_ref().and_then(|s| s.map_source()) } else { None };
                #[cfg(feature = "baked-tile")]
                let baked_src = SliceSource(TILE);
                let map_src: Option<&dyn ByteSource> = match &sd_src {
                    Some(s) => Some(s),
                    #[cfg(feature = "baked-tile")]
                    None => Some(&baked_src),
                    #[cfg(not(feature = "baked-tile"))]
                    None => None,
                };

                // A transient SD read failure makes the reader build fail; skip the redraw and
                // keep the last buffer (like the route path), rather than show a half-read map.
                if let Some(reader) = map_src.and_then(|s| Reader::new(s, map_cache).ok()) {
                    // Render *only* the map plane into Layer 1 — the overlay (hold ring) is drawn
                    // onto Layer 2 below and composited by the LTDC, so the map buffer never carries
                    // the bulge and is re-rendered only when the map itself changes.
                    let stats = app.render_map(&mut fb, &reader, route.as_ref(), W as f32, H as f32, color_fn);
                    let render_us = t0.saturating_elapsed().as_micros();
                    // Snapshot this frame's render stats for the host telemetry line (the same
                    // numbers as the RTT `map frame` log / the sim's Render Stats panel).
                    #[cfg(feature = "debug-usb")]
                    {
                        last_telem = obc_platform::debug_usb::Telemetry {
                            frame_us: render_us as u32,
                            lod: stats.lod as u8,
                            feat_drawn: stats.features_drawn as u32,
                            feat_tried: stats.features_tried as u32,
                            feat_dropped: stats.features_dropped as u32,
                            chunks: stats.chunks_visited as u32,
                            cache_hits: stats.map_chunk_hits,
                            cache_misses: stats.map_chunk_misses,
                            sd_reads: stats.map_sd_reads,
                            bytes_read: stats.map_bytes_read,
                        };
                    }

                    // Flip to the freshly-drawn buffer at the next vblank. Swap front/back roles
                    // only once the reload is confirmed landed; on a timeout the LTDC is still
                    // scanning the old front, so we keep the buffers (the next redraw re-renders
                    // into this same back and retries) rather than render into the scanned-out one.
                    if flip_to(LtdcLayer::Layer1, back_addr) {
                        core::mem::swap(&mut front_addr, &mut back_addr);
                    } else {
                        defmt::warn!("LTDC: Layer 1 vblank reload didn't land in 50 ms — kept buffers, skipped swap");
                    }
                    // Chunk-cache hit rate + SD-read overhead this frame (issue #37's measured
                    // deliverables). `RenderStats` reports the per-frame delta over the persistent
                    // cache, so this tracks what each redraw actually pulled off the card.
                    let reqs = stats.map_chunk_hits + stats.map_chunk_misses;
                    let hit_pct = (stats.map_chunk_hits * 100).checked_div(reqs).unwrap_or(0);
                    defmt::debug!(
                        "map frame: {=u64} us | lod {=usize} | feat {=usize}/{=usize} | map-cache {=u32}% hit, {=u32} rd, {=u32} B",
                        render_us,
                        stats.lod,
                        stats.features_drawn,
                        stats.features_tried,
                        hit_pct,
                        stats.map_sd_reads,
                        stats.map_bytes_read
                    );
                } else {
                    // `Reader::new` failed (flaky SD). The `dirty.map` edge we drained above would
                    // otherwise be lost, freezing the map; latch it so next frame retries the redraw
                    // until the reader builds (issue #66).
                    pending_map_redraw = true;
                    defmt::warn!(
                        "map: reader build failed this frame (flaky SD?) — kept buffers, will retry redraw next frame"
                    );
                }
            }

            // Overlay plane (Layer 2) — **single-executor build only**. `dirty.overlay` is true
            // whenever the hold bulge has live content this frame, plus the one trailing frame when
            // it goes quiet (`App::take_dirty` folds in the old `overlay_was_active` edge, so the
            // layer is cleared once when the bulge ends). This never touches the map — the LTDC blends
            // Layer 2 over whatever Layer 1 already shows — so an animating ring over a static map
            // re-renders only this small layer (the `map frame` log above stays silent while these
            // `overlay frame`s tick by). Like the map plane it is double-buffered + vblank-flipped:
            // render the whole overlay (clear to transparent, then the bulge) into the *back* buffer,
            // then flip — drawing in place into the scanned buffer tears the bulge as the clear races
            // the scan. (In the two-plane build this all runs on the high-priority `input_overlay_task`
            // instead, so the map plane here never re-renders the overlay.)
            #[cfg(feature = "single-executor")]
            if dirty.overlay {
                let t0 = Instant::now();
                // Render into the back overlay buffer (not the one being scanned out).
                // SAFETY: overlay_back is the buffer the LTDC is *not* scanning; the only live
                // `&mut` over it, dropped before the flip.
                let back = unsafe { core::slice::from_raw_parts_mut(overlay_back as *mut u16, FB_PIXELS) };
                let mut overlay_fb = FramebufferArgb4444::new(back, W as u32, H as u32);
                overlay_fb.clear_transparent();
                app.render_overlay(&mut overlay_fb, W as f32, H as f32, color_fn);
                let overlay_us = t0.saturating_elapsed().as_micros();

                // Flip Layer 2 to the freshly-drawn buffer at the next vblank, swapping only once
                // the reload lands (same tear-free contract as the map flip). On a timeout keep the
                // buffers and retry next frame.
                if flip_to(LtdcLayer::Layer2, overlay_back) {
                    core::mem::swap(&mut overlay_front, &mut overlay_back);
                } else {
                    defmt::warn!("LTDC: Layer 2 vblank reload didn't land in 50 ms — kept buffers, skipped swap");
                }
                defmt::debug!("overlay frame: {=u64} us | active {=bool}", overlay_us, app.overlay_active());
            }

            // Publish render-stats telemetry host-ward at ~2 Hz (issue #38): the last map frame's
            // numbers (render time / LOD / features / chunks / map-cache + SD), the same data the
            // RTT `map frame` log carries. Throttled here (not in the USB task) so the link never
            // floods and the device never stalls on it.
            #[cfg(feature = "debug-usb")]
            if now.wrapping_sub(last_telem_ms) >= 500 {
                last_telem_ms = now;
                obc_platform::debug_usb::set_telemetry(last_telem);
            }

            Timer::after_millis(LOOP_MS).await;
        }
    }
}
