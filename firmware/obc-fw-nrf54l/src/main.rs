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
//!   N0. crate skeleton + embassy bring-up: blinky + RTT + this peripheral plan  <- this commit (#121)
//!   N1. `Panel` HAL + ST7789 SPIM backend + banded RGB222→RGB565 push + glass demo (#122)
//!   N2. microSD on a dedicated SPIM (reuse obc-platform's FatFs byte adapters)   (#123)
//!   N3. board memory profile (host-vs-nRF) + budget assert                        (#124)
//!   N4. RGB222 full framebuffer + full map on glass (retire `Framebuffer565`)     (#125)
//!   N5. buttons + two-plane InterruptExecutor + fluid composite-on-push bulge     (#126)
//!   N6. debug/sensor stream over VCOM UART + load→ride→save-GPX = PARITY          (#127)
//!   N7. docs + CI (add nRF; drop STM32 from the required check)                   (#128)
//!
//! Clock: the M33 application core runs at 128 MHz; embassy-time is driven by the **GRTC**
//! (Global RTC) via the `time-driver-grtc` feature — the nRF54L has no legacy RTC time-driver.
//!
//! ============================ Peripheral / pin plan ============================
//! The allocation below is fixed now so later phases just wire to it. Pin names are the
//! embassy-nrf `P{port}_{pin}` form (e.g. `P2_09` = GPIO port 2, pin 9). LED/button/VCOM
//! assignments are the nRF54L15-DK's, taken from Zephyr's `nrf54l15dk` board DTS; the SPIM
//! pin sets are the DK's default `spi00`/`spi22` pinctrl. CS/DC/RST GPIOs for the panel and
//! the SD card are assigned in N1/N2 (left out here to avoid pinning them prematurely).
//!
//! ## On-board LEDs (active-HIGH) — Zephyr `led0..3`
//!   LED0 P2_09 | LED1 P1_10 | LED2 P2_07 | LED3 P1_14
//! N0 blinks **LED0 (P2_09)** as the liveness proof (same pin as embassy's nRF54L blinky).
//!
//! ## Push-buttons (active-LOW, internal pull-up) — Zephyr `sw0..3`, the UI input (#126)
//!   BTN0 P1_13 | BTN1 P1_09 | BTN2 P1_08 | BTN3 P0_04
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture
//! recogniser, exactly like the STM32's four GPIO buttons. (PREV/NEXT/SELECT/BACK roles
//! assigned in N5.) Note these are read via the **GPIOTE** peripheral (the `gpiote` feature).
//!
//! ## Display SPIM — ST7789 EYESPI (#122)
//!   Instance **SERIAL00 / SPIM00** — the *only* instance that reaches 32 MHz (it lives in the
//!   fast peripheral power domain); the ST7789 wants a fast bus, so it gets this one.
//!   DK default pins: SCK P2_01 | MOSI P2_02 | MISO P2_04   (+ CS / DCX / RST GPIOs in N1)
//!   MISO is unused for the write-only panel but the bus owns the pin. Band push expands the
//!   RGB222 framebuffer → RGB565 and SPIM-DMAs a CASET/RASET window (the wire format lives in
//!   `Panel::flush_band`, not the framebuffer — the same seam the future FLPR/LS021B7DD02 reuses).
//!
//! ## microSD SPIM — map/route/track storage (#123)
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz), *separate*
//!   from the display bus, on its own software CS. DK expansion-header SPI pins:
//!   SCK P1_11 | MISO P1_07 | MOSI P1_06   (+ CS GPIO in N2)
//!   The EYESPI connector also carries a microSD that *shares the display bus*; we leave that slot
//!   **unpopulated** and use this dedicated SPIM instead (a clean reuse of the STM32's standalone
//!   SD-over-SPI reader + obc-platform's FatFs adapters). P1_06/P1_07 alias VCOM's unused RTS/CTS
//!   below — no conflict, since the VCOM link is 2-wire (TX/RX only).
//!
//! ## VCOM UARTE — debug-sensor / telemetry stream (#127)
//!   Instance **SERIAL20 / UARTE20**, the DK's `chosen` console wired to the onboard J-Link's
//!   USB-CDC VCOM: TX P1_04 | RX P1_05  (RTS P1_06 / CTS P1_07 available, unused).
//!   The nRF54L15 has **no USB peripheral**, so — unlike the STM32's second USB-CDC port — the fake
//!   GPS/baro/compass feed and ride telemetry ride this UART; defmt logs ride RTT on the same cable.
//!   obc-platform's debug-source protocol is transport-agnostic, so it moves over from USB unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor (#126)
//!   The two-plane architecture runs input + the overlay on a high-priority `InterruptExecutor`
//!   that preempts the map render. On STM32 that executor was pended from the unused UART5 vector;
//!   the nRF analog is a dedicated **software-interrupt vector**: reserve **SWI00** for it (the M33
//!   also has SWI01/02/03 + EGU10/EGU20 free). It pends above thread mode but below the P0 GRTC
//!   time-driver, so `Timer`s still wake mid-render.
//!
//! ## Flash / RAM
//!   From the `nrf54l15-app-s` `memory.x`: FLASH 1524K @ 0x0000_0000, RAM 256K @ 0x2000_0000.
//!   A future MCUboot retrofit re-partitions flash — don't hard-code flash assumptions (see
//!   `memory.x` and epic #120). RAM is tight (no external SDRAM, unlike the STM32 prototype): the
//!   single RGB222 framebuffer is ~75 KB and the renderer scratch + caches must fit the rest —
//!   the board memory profile + budget assert is N3 (#124).
//! =============================================================================

#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    // LED0 (DK silkscreen "LED0", green) on P2_09, active-high. The blink + the RTT log
    // below are the N0 liveness proof: clocks, the GRTC time-driver, GPIO, and the defmt/RTT
    // transport are all up if this toggles at ~1.6 Hz with matching log lines over the J-Link.
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);

    info!("obc-fw-nrf54l N0: bring-up alive — M33 @128 MHz, GRTC time-driver, defmt/RTT");

    let mut tick: u32 = 0;
    loop {
        led.set_high();
        info!("LED0 on  (tick {})", tick);
        Timer::after_millis(300).await;

        led.set_low();
        info!("LED0 off (tick {})", tick);
        Timer::after_millis(300).await;

        tick = tick.wrapping_add(1);
    }
}
