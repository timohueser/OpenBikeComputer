use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::{bind_interrupts, config, cracen, mode::Blocking};
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    SWI00 => mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => mpsl::ClockInterruptHandler;
    RADIO_0 => mpsl::HighPrioInterruptHandler;
    TIMER10 => mpsl::HighPrioInterruptHandler;
    GRTC_3 => mpsl::HighPrioInterruptHandler;
});

#[embassy_executor::task]
async fn radio(mpsl: &'static mpsl::MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

pub struct Controls {
    pub buttons: [Input<'static>; 4],
    pub leds: [Output<'static>; 4],
}

pub fn init(spawner: Spawner) -> (sdc::SoftdeviceController<'static>, Controls, [u8; 6]) {
    let mut config = config::Config::default();
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    // MPSL calibrates the RC; no LFXO load-capacitor setup is required.
    config.lfclk_source = config::LfclkSource::InternalRC;
    let p = embassy_nrf::init(config);
    let controls = Controls {
        buttons: [
            Input::new(p.P1_13, Pull::Up),
            Input::new(p.P1_09, Pull::Up),
            Input::new(p.P1_08, Pull::Up),
            Input::new(p.P0_04, Pull::Up),
        ],
        leds: [
            Output::new(p.P2_09, Level::Low, OutputDrive::Standard),
            Output::new(p.P1_10, Level::Low, OutputDrive::Standard),
            Output::new(p.P2_07, Level::Low, OutputDrive::Standard),
            Output::new(p.P1_14, Level::Low, OutputDrive::Standard),
        ],
    };
    let mpsl_p = mpsl::Peripherals::new(
        p.GRTC_CH7,
        p.GRTC_CH8,
        p.GRTC_CH9,
        p.GRTC_CH10,
        p.GRTC_CH11,
        p.TIMER10,
        p.TIMER20,
        p.TEMP,
        p.PPI10_CH0,
        p.PPI20_CH1,
        p.PPIB11_CH0,
        p.PPIB21_CH0,
    );
    let lfclk = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: 500,
        skip_wait_lfclk_started: false,
    };
    static MPSL: StaticCell<mpsl::MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk)));
    spawner.spawn(unwrap!(radio(mpsl)));
    let sdc_p = sdc::Peripherals::new(
        p.PPI00_CH1,
        p.PPI00_CH3,
        p.PPI10_CH1,
        p.PPI10_CH2,
        p.PPI10_CH3,
        p.PPI10_CH4,
        p.PPI10_CH5,
        p.PPI10_CH6,
        p.PPI10_CH7,
        p.PPI10_CH8,
        p.PPI10_CH9,
        p.PPI10_CH10,
        p.PPI10_CH11,
        p.PPIB00_CH1,
        p.PPIB00_CH2,
        p.PPIB00_CH3,
        p.PPIB10_CH1,
        p.PPIB10_CH2,
        p.PPIB10_CH3,
    );
    static RNG: StaticCell<cracen::Cracen<'static, Blocking>> = StaticCell::new();
    let rng = RNG.init(cracen::Cracen::new_blocking(p.CRACEN));
    // Scratch capacity; Builder checks the actual controller requirement at boot.
    static MEM: StaticCell<sdc::Mem<8192>> = StaticCell::new();
    let mem = MEM.init(sdc::Mem::new());
    let builder = unwrap!(sdc::Builder::new()).support_adv().support_ext_adv().support_peripheral();
    let builder = unwrap!(builder.peripheral_count(1));
    defmt::info!("controller memory: {} bytes", unwrap!(builder.required_memory()));
    let controller = unwrap!(builder.build(sdc_p, rng, mpsl, mem));
    let low = embassy_nrf::pac::FICR.deviceaddr(0).read().to_le_bytes();
    let high = embassy_nrf::pac::FICR.deviceaddr(1).read().to_le_bytes();
    (controller, controls, [low[0], low[1], low[2], low[3], high[0], high[1]])
}
