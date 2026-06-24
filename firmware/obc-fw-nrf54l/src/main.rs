//! nRF54L15-DK board firmware for OpenBikeComputer — the **real hardware target**.
//!
//! Unlike the STM32F429 prototype (a bridge that made the HAL seams concrete), the
//! nRF54L15 + ST7789 EYESPI panel is what the project ships on. This crate ports the
//! shared `obc-app` onto it to STM32-prototype parity (load route → ride → save GPX on
//! glass, fake-sensor fed). Nothing app-facing lives here: `obc-render` / `obc-app` /
//! `obc-reader` / `obc-route` + `obc-platform` stay board-agnostic; only the nRF HAL
//! wiring + the ST7789 `Panel` backend are board-specific. See epic #120.
//!
//! Bring-up is phased so each hardware layer is verified (over defmt/RTT, and on glass via
//! the webcam capture at `/tmp/obc-cam/panel.jpg`) before the next is stacked:
//!   N0. crate skeleton + embassy bring-up: blinky + RTT + this peripheral plan          (#121)
//!   N1. `Panel` HAL + ST7789 SPIM backend + banded RGB222→RGB565 push + glass demo  <- this commit (#122)
//!   N2. microSD on a dedicated SPIM (reuse obc-platform's FatFs byte adapters)           (#123)
//!   N3. board memory profile (host-vs-nRF) + budget assert                              (#124)
//!   N4. RGB222 full framebuffer + full map on glass (retire `Framebuffer565`)           (#125)
//!   N5. buttons + two-plane InterruptExecutor + fluid composite-on-push bulge           (#126)
//!   N6. debug/sensor stream over VCOM UART + load→ride→save-GPX = PARITY                (#127)
//!   N7. docs + CI (add nRF; drop STM32 from the required check)                         (#128)
//!
//! Clock: the M33 application core runs at 128 MHz; embassy-time is driven by the **GRTC**
//! (Global RTC) via the `time-driver-grtc` feature — the nRF54L has no legacy RTC time-driver.
//!
//! ============================ Peripheral / pin plan ============================
//! Pin names are the embassy-nrf `P{port}_{pin}` form (e.g. `P2_09` = GPIO port 2, pin 9).
//! LED/button/VCOM/SPI assignments are the nRF54L15-DK's, from Zephyr's `nrf54l15dk` DTS and
//! the DK HW user guide pin maps (Tables 3–5). The three GPIO ports have different reach: P2 =
//! MCU domain (fast, ≤64 MHz, the SERIAL00 home), P1 = PERI domain (≤8 MHz), P0 = LP domain.
//!
//! ## On-board LEDs (active-HIGH) — Zephyr `led0..3`
//!   LED0 P2_09 | LED1 P1_10 | LED2 P2_07 | LED3 P1_14
//! N1 blinks **LED0 (P2_09)** once per drawn frame as a liveness heartbeat.
//!
//! ## Push-buttons (active-LOW, internal pull-up) — Zephyr `sw0..3`, the UI input (#126)
//!   BTN0 P1_13 | BTN1 P1_09 | BTN2 P1_08 | BTN3 P0_04
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture
//! recogniser, exactly like the STM32's four GPIO buttons. (PREV/NEXT/SELECT/BACK roles
//! assigned in N5.) Read via the **GPIOTE** peripheral (the `gpiote` feature). These are the
//! DK's own buttons — no jumpers — and they stay free because the display lives on P2 (below).
//!
//! ## Display SPIM — ST7789 EYESPI stand-in (#122)
//!   Instance **SERIAL00 / SPIM00** — the only instance that reaches 32 MHz (fast/MCU power
//!   domain, port P2); the panel wants a fast bus so it gets this one. Its pins are the DK's
//!   on-board QSPI-flash bus (P2.00–P2.05). We never use that flash (maps live on SD), so the
//!   **Board Configurator** app electronically disconnects it ("external memory → GPIO on the
//!   P2 header") and routes the pins out — no soldering on current board revisions. The whole
//!   panel then sits on the P2 header:
//!     SCK P2_01 | MOSI P2_02 | CS P2_05 | DC P2_03 | RST P2_00   (MISO P2_04 unused, write-only)
//!   CS is held low (single device on the bus; embassy-nrf drives no hardware CS). The panel's
//!   level shifters want 3–5 V logic, so the DK I/O rail must be raised from its 1.8 V default
//!   to **3.3 V** (VDDM, also in the Board Configurator — HW guide §2.2.1); Vin is fed from the
//!   DK's 5 V (VBUS) so the panel's onboard 3.3 V LDO keeps headroom. Putting the display on P2
//!   leaves all of P1 free for SD (N2) + VCOM (N6) + the buttons. Band push expands the RGB222
//!   framebuffer → RGB565 and SPIM-DMAs a CASET/RASET window (the wire format lives in
//!   `Panel::flush_band`, the same seam the future FLPR/LS021B7DD02 reuses); that lands at N4.
//!   N1 just streams a colour-bar test pattern to prove wiring + init + addressing.
//!
//! ## microSD SPIM — map/route/track storage (#123)
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz),
//!   *separate* from the display bus, on its own software CS. DK expansion-header SPI pins:
//!   SCK P1_11 | MISO P1_07 | MOSI P1_06   (+ CS GPIO in N2)
//!   The EYESPI connector also carries a microSD that *shares the display bus*; we leave that
//!   slot **unpopulated** and use this dedicated SPIM instead (a clean reuse of the STM32's
//!   standalone SD-over-SPI reader + obc-platform's FatFs adapters). P1_06/P1_07 alias VCOM's
//!   unused RTS/CTS below — no conflict, since the VCOM link is 2-wire (TX/RX only).
//!
//! ## VCOM UARTE — debug-sensor / telemetry stream (#127)
//!   Instance **SERIAL20 / UARTE20**, the DK's `chosen` console wired to the onboard J-Link's
//!   USB-CDC VCOM: TX P1_04 | RX P1_05  (RTS P1_06 / CTS P1_07 available, unused).
//!   The nRF54L15 has **no USB peripheral**, so — unlike the STM32's second USB-CDC port — the
//!   fake GPS/baro/compass feed and ride telemetry ride this UART; defmt logs ride RTT on the
//!   same cable. obc-platform's debug-source protocol is transport-agnostic, so it moves over
//!   from USB unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor (#126)
//!   The two-plane architecture runs input + the overlay on a high-priority `InterruptExecutor`
//!   that preempts the map render. On STM32 that executor was pended from the unused UART5
//!   vector; the nRF analog is a dedicated **software-interrupt vector**: reserve **SWI00** for
//!   it (the M33 also has SWI01/02/03 + EGU10/EGU20 free). It pends above thread mode but below
//!   the P0 GRTC time-driver, so `Timer`s still wake mid-render.
//!
//! ## Flash / RAM
//!   From the `nrf54l15-app-s` `memory.x`: FLASH 1524K @ 0x0000_0000, RAM 256K @ 0x2000_0000.
//!   A future MCUboot retrofit re-partitions flash — don't hard-code flash assumptions (see
//!   `memory.x` and epic #120). RAM is tight (no external SDRAM, unlike the STM32 prototype):
//!   the single RGB222 framebuffer is ~75 KB and the renderer scratch + caches must fit the
//!   rest — the board memory profile + budget assert is N3 (#124).
//! =============================================================================

#![no_std]
#![no_main]

mod st7789;

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::{Delay, Timer};
use st7789::{St7789, HEIGHT, WIDTH};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
});

/// 8-bar palette in big-endian RGB565 (what the ST7789 RAMWR stream expects):
/// white, yellow, cyan, green, magenta, red, blue, black.
const BARS: [u16; 8] = [0xFFFF, 0xFFE0, 0x07FF, 0x07E0, 0xF81F, 0xF800, 0x001F, 0x0000];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    // LED0 (P2_09) heartbeat — toggles once per drawn frame so liveness is visible even before
    // looking at the panel.
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);

    // Display control lines on the (flash-freed) P2 header. CS is asserted low once and held —
    // single device on the bus, the same trick the STM32 panel uses; embassy-nrf's Spim drives
    // no hardware CS. RST idles high (released).
    let _cs = Output::new(p.P2_05, Level::Low, OutputDrive::Standard);
    let dc = Output::new(p.P2_03, Level::Low, OutputDrive::Standard);
    let rst = Output::new(p.P2_00, Level::High, OutputDrive::Standard);

    // SERIAL00 as a write-only SPIM: the panel never talks back, so MISO (P2_04) is omitted.
    // 8 MHz is comfortable over the jumpered bring-up bus (a full 240×320 frame ≈ 153 ms);
    // SERIAL00 reaches 32 MHz on a clean board — worth revisiting once the panel is on a PCB.
    let mut config = spim::Config::default();
    config.frequency = spim::Frequency::M8;
    let spi = spim::Spim::new_txonly(p.SERIAL00, Irqs, p.P2_01, p.P2_02, config);

    info!("obc-fw-nrf54l N1: ST7789 bring-up — SPIM00 @8MHz on P2.01/P2.02, DC P2.03, RST P2.00, CS P2.05");

    let mut panel = St7789::new(spi, dc, rst, Delay);
    panel.init();
    info!("ST7789 init done ({}x{}), streaming colour bars", WIDTH, HEIGHT);

    // Scrolling colour bars: every frame the 8-bar palette rotates by one, so the bars march
    // sideways — an unmistakable "alive and addressed correctly" signal on the webcam.
    let bar_w = WIDTH / BARS.len() as u16;
    let mut frame: usize = 0;
    loop {
        // Vertical bars → every row is identical, so build one row and stream it HEIGHT times.
        let mut row = [0u8; WIDTH as usize * 2];
        for x in 0..WIDTH {
            let bar = (x / bar_w) as usize % BARS.len();
            let c = BARS[(bar + frame) % BARS.len()];
            let i = x as usize * 2;
            row[i] = (c >> 8) as u8;
            row[i + 1] = c as u8;
        }

        panel.set_window(0, 0, WIDTH - 1, HEIGHT - 1);
        for _ in 0..HEIGHT {
            panel.push(&row);
        }

        led.toggle();
        info!("frame {} drawn (bars shifted {})", frame, frame % BARS.len());
        Timer::after_millis(700).await;
        frame = frame.wrapping_add(1);
    }
}
