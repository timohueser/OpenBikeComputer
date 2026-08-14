//! nRF54LM20-DK pin, electrical, peripheral, and interrupt contract.
//!
//! Pin names use embassy-nRF's `P{port}_{pin}` form. P2 is the fast MCU domain used by the FLPR;
//! P1 is the PERI domain; P0 is the low-power domain. Service/task composition stays beside its
//! owning driver; boot policy, move-only input construction, board assignments and IRQ policy live here.
//!
//! ## User input and diagnostics
//!
//! | Role | Peripheral / pin | Electrical intent |
//! | --- | --- | --- |
//! | heartbeat LED1 | P1_25 | active high, standard drive |
//! | up / BTN0 | P1_26 | active low, internal pull-up |
//! | down / BTN1 | P1_09 | active low, internal pull-up |
//! | select / BTN3 | P0_05 | active low, internal pull-up |
//! | back / BTN2 | P1_08 | active low, internal pull-up |
//! | debug VCOM TX | SERIAL20 / P1_16 | UARTE20, 8N1 at 115200, no flow control |
//! | debug VCOM RX | SERIAL20 / P1_17 | UARTE20, 8N1 at 115200, no flow control |
//! | sensor SDA | SERIAL22 / P1_04 | TWIM22, 400 kHz, pull-up enabled |
//! | sensor SCL | SERIAL22 / P1_03 | TWIM22, 400 kHz, pull-up enabled |
//! | sensor TX-ready | P1_05 | active high, internal pull-down |
//!
//! The DK's VCOM hardware flow control must remain disabled. RTT carries defmt independently.
//!
//! ## Display and native-SD sharing
//!
//! The LS021 panel runs at 3.3 V VDDM and is scanned by the FLPR. The native four-bit sEMMC image
//! fixes the card pads, so the display shares only D3/D1 and never claims the four card-only pads.
//!
//! ```text
//! sEMMC:   P2_00 D3   P2_01 CLK  P2_02 D0  P2_03 D2  P2_04 D1  P2_05 CMD
//! display: P2_06 R0   P2_08 R1   P2_09 G0  P2_10 G1  P2_00 B0* P2_04 B1*
//!          P2_07 BCK  P1_14 BSP   (* CTRLSEL switches between GPIO and VPR ownership)
//! gate:    P1_10 GSP  P1_11 GCK  P1_12 GEN P1_13 INTB
//! COM:     P1_22 VCOM P1_23 VB   P1_24 VA
//! ```
//!
//! The Board Configurator disconnects the on-DK QSPI device from P2_00..P2_05. Card-only pads are
//! configured by `semmc`; claiming them as embassy GPIO would create two owners. COM is M33-driven
//! by default and uses TIMER/DPPI/GPIOTE with `com-hw`, on the same P1_22..P1_24 nets.
//!
//! ## Interrupt ladder
//!
//! - P0: GRTC time driver and MPSL's timing-critical radio lane.
//! - P1: embassy peripheral default, including VCOM and VPR00; MPSL SWI00/CLOCK_POWER with BLE.
//! - P3: SWI01 input/overlay executor and the SERIAL22 sensor bus.
//! - thread mode: ride loop, BLE task, and sensor task.
//!
//! SWI01 is deliberately stable across builds because SWI00 belongs to MPSL. VPR00 is the sEMMC
//! completion vector and is re-armed during every soft-peripheral boot.

use embassy_nrf::interrupt;

#[cfg(feature = "debug-uart")]
use embassy_nrf::buffered_uarte;
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use embassy_nrf::twim;
#[cfg(any(feature = "debug-uart", not(feature = "synth")))]
use embassy_nrf::{bind_interrupts, peripherals};

/// sEMMC completion event: VEVIF event 20 is routed to `VPR00_IRQn` by the FLPR firmware.
#[interrupt]
unsafe fn VPR00() {
    crate::semmc::on_vpr00_irq();
}

// VCOM UARTE20 RX/TX interrupt binding.
#[cfg(feature = "debug-uart")]
bind_interrupts!(pub(crate) struct UartIrqs {
    SERIAL20 => buffered_uarte::InterruptHandler<peripherals::SERIAL20>;
});

// TWIM22 sensor-bus interrupt binding.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
bind_interrupts!(pub(crate) struct SensorIrqs {
    SERIAL22 => twim::InterruptHandler<peripherals::SERIAL22>;
});

/// Run the M33 at 128 MHz: embassy's CK64 default materially regresses the CPU-bound map render.
/// MPSL radio timing requires the external HF crystal. LFCLK deliberately remains on the calibrated
/// internal RC because the unprogrammed LFXO load caps put its crystal off-frequency and cause HCI
/// 0x3E connection failures; see `ble.rs`.
macro_rules! init {
    () => {{
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        #[cfg(feature = "ble")]
        {
            config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
            config.lfclk_source = embassy_nrf::config::LfclkSource::InternalRC;
        }
        embassy_nrf::init(config)
    }};
}
pub(crate) use init;

/// Read and clear the secure RESET block's write-one-to-clear reason register.
macro_rules! take_reset_reason {
    () => {{
        const RESETREAS: *mut u32 = 0x5010_E600 as *mut u32;
        let v = unsafe { RESETREAS.read_volatile() };
        unsafe { RESETREAS.write_volatile(v) };
        v
    }};
}
pub(crate) use take_reset_reason;

/// Start or re-adopt the one-handle watchdog used across app and bootloader warm resets.
///
/// It counts through sleep, pauses under debugger halt, and deliberately leaves a foreign
/// configuration unfed so that it expires once and the next boot can start clean. The caller passes
/// `obc_dfu::WDT_TIMEOUT_TICKS`, keeping the app and bootloader's warm-reset configuration identical.
macro_rules! watchdog {
    ($peripheral:expr, $timeout_ticks:expr) => {{
        let mut cfg = embassy_nrf::wdt::Config::default();
        cfg.timeout_ticks = $timeout_ticks;
        cfg.action_during_debug_halt = embassy_nrf::wdt::HaltConfig::Pause;
        embassy_nrf::wdt::Watchdog::try_new::<_, 1>($peripheral, cfg)
    }};
}
pub(crate) use watchdog;

// The move-only HAL values stay constructed at their original call sites: this macro is only the
// board-owned input assignment/electrical policy, with no call frame, singleton, or reordered init.
macro_rules! input_hardware {
    (buttons $p:ident) => {{
        use embassy_nrf::interrupt::InterruptExt as _;
        let buttons = obc_platform::ButtonInput::new(
            embassy_nrf::gpio::Input::new($p.P1_26, embassy_nrf::gpio::Pull::Up),
            embassy_nrf::gpio::Input::new($p.P1_09, embassy_nrf::gpio::Pull::Up),
            embassy_nrf::gpio::Input::new($p.P0_05, embassy_nrf::gpio::Pull::Up),
            embassy_nrf::gpio::Input::new($p.P1_08, embassy_nrf::gpio::Pull::Up),
        );
        embassy_nrf::interrupt::SWI01.set_priority(embassy_nrf::interrupt::Priority::P3);
        buttons
    }};
    (uart $p:ident, $rx_buf:expr, $tx_buf:expr) => {
        embassy_nrf::buffered_uarte::BufferedUarte::new(
            $p.SERIAL20,
            $p.P1_17,
            $p.P1_16,
            $crate::board::UartIrqs,
            embassy_nrf::uarte::Config::default(),
            $rx_buf,
            $tx_buf,
        )
    };
    (sensors $p:ident, $tx_buf:expr) => {{
        use embassy_nrf::interrupt::InterruptExt as _;
        let mut config = embassy_nrf::twim::Config::default();
        config.frequency = embassy_nrf::twim::Frequency::K400;
        config.sda_pullup = true;
        config.scl_pullup = true;
        let twim =
            embassy_nrf::twim::Twim::new($p.SERIAL22, $crate::board::SensorIrqs, $p.P1_04, $p.P1_03, config, $tx_buf);
        embassy_nrf::interrupt::SERIAL22.set_priority(embassy_nrf::interrupt::Priority::P3);
        let txready = embassy_nrf::gpio::Input::new($p.P1_05, embassy_nrf::gpio::Pull::Down);
        (twim, txready)
    }};
}
pub(crate) use input_hardware;
