//! GPS (u-blox SAM-M10Q), barometric altimeter (Bosch BMP581) and compass (the AK09916
//! magnetometer inside a TDK ICM-20948) on one shared I²C bus: the board transport and the
//! event-driven sensor task.
//!
//! All three chips sit on TWIM22 on P1 (SDA P1.04 / SCL P1.03, the clock-capable pin with SDA
//! adjacent); the GPS TX-Ready line is the single interrupt (P1.05). The decode is host-tested in
//! [`obc_sensors`]; this module owns the concrete `Twim` transactions and the [`sensor_task`] that
//! coalesces a GPS fix, a coincident baro reading and a heading into one datapoint and publishes it
//! through its [`SensorTaskLink`].
//!
//! Only the ICM-20948's three magnetometer axes are used; the accel and gyro stay asleep. The
//! AK09916 is reached by putting the ICM in I²C bypass, so it answers directly at `0x0C` as if it
//! were a standalone 3-axis compass.
//!
//! The task waits on the TX-Ready edge and does no bus work between fixes, but the wait also has a
//! timeout at about the fix interval: if no edge arrives, the task polls the DDC anyway, so the GPS
//! still works at the fix rate with TX-Ready dead. A NAV-PVT with no fix publishes nothing, so
//! `LocationSource::poll` returns `None` and the camera never teleports.
//!
//! After boot acquisition the task follows the app's [`SensorDemand`]. With no recording or
//! position request it sends a `CFG-RST` controlled GNSS stop and parks without DDC polling; an
//! open Peak View keeps the compass sampling. A controlled GNSS start retains receiver
//! configuration and navigation data. This is not backup sleep, and idle current is not measured.

use defmt::{debug, error, info, warn};
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_nrf::gpio::Input;
use embassy_nrf::twim::Twim;
use embassy_time::{Duration, Instant, Timer};
use obc_platform::sensor_hub::{GpsPower, SensorDemand, SensorPresence, SensorTaskLink};
use obc_sensors::{bmp581, compass, icm20948, ubx};

/// SAM-M10Q I²C (DDC) slave address.
const M10_ADDR: u8 = 0x42;
/// DDC registers: `0xFD/0xFE` is a 16-bit big-endian count of the bytes pending in the message
/// buffer; `0xFF` is the data stream, and a read of it does not auto-increment, so a burst drains
/// the buffer.
const DDC_COUNT_REG: u8 = 0xFD;
const DDC_DATA_REG: u8 = 0xFF;

/// Default GPS fix interval at boot (seconds). The ride loop pushes the persisted setting through
/// the sensor hub's rate latch as soon as it loads settings, so this governs only the first second.
const DEFAULT_INTERVAL_S: u16 = 1;

/// TX-Ready config: the module PIO wired to P1.05, active-high, asserting when about one NAV-PVT
/// is pending (`THRESHOLD × 8`). Verify the PIO number against the SAM-M10Q datasheet on a new
/// board; the DDC-poll timeout below keeps fixes flowing if TX-Ready never fires.
const TXREADY_PIO: u8 = 6;
const TXREADY_THRESHOLD: u16 = 12;

/// Extra slop over the fix interval for the DDC-poll timeout, so a full NAV-PVT has finished
/// streaming before a fallback poll reads it.
const DEADLINE_MARGIN_MS: u64 = 300;

/// Persistent UBX byte accumulator: the DDC may hand back a NAV-PVT split across reads, so
/// unparsed tail bytes carry to the next read. 300 B holds about three NAV-PVTs plus slack.
const ACC_CAP: usize = 300;

/// Bound on the boot-fix acquisition. The first fix sets the clock and warms the ephemeris, but a
/// boot under cover must still drop into the power-managed steady state.
const BOOT_ACQUIRE_TIMEOUT_S: u64 = 150;

/// Per-board hard-iron offset (µT) subtracted from each magnetometer axis before the heading.
/// Nearby steel or magnets shift the field by a fixed vector and skew the heading. Zero until a
/// calibration routine fills it. TODO: rotate-the-device calibration.
const HARD_IRON_UT: (f32, f32, f32) = (0.0, 0.0, 0.0);

/// Magnetic declination (degrees east), which turns the magnetic heading into the true-north
/// heading the GPS course and the map use. `0.0` is the raw magnetic heading. TODO: derive the
/// local value from the GPS position.
const DECLINATION_DEG: f32 = 0.0;

/// Conversion budget for one AK09916 single measurement: poll DRDY this many times at
/// [`MAG_POLL_MS`] before reading the sample anyway, because the data registers hold the last
/// conversion whatever the bit says.
const MAG_POLL_TRIES: u8 = 10;
const MAG_POLL_MS: u64 = 3;

/// Compass update period (ms) while stationary. The heading is never logged, so it is decoupled
/// from the GPS fix rate, which is tuned for power and logging. It is read only while stationary
/// and tracking; while moving the GPS course is the heading, and when idle the receiver is asleep.
const COMPASS_INTERVAL_MS: u64 = 200;

/// Heading dead-band (degrees): publish a new heading only once it has moved at least this much.
/// Without it a noisy magnetometer would dispatch about five headings a second while the device is
/// held still, and each one repaints the heading-up map.
const HEADING_DEADBAND_DEG: f32 = 2.0;

#[derive(Default)]
struct FixState {
    had_fix: bool,
    txready_seen: bool,
    /// So the poll-fallback notice logs once, not every cycle.
    noted_poll_fallback: bool,
    /// De-dup a re-read of the same epoch (a fallback poll can re-read it).
    last_itow: Option<u32>,
    /// Whether the latest valid fix was stationary (no GPS course). The app uses the magnetometer
    /// heading only when stopped, so the task reads it only then.
    stationary: bool,
    /// Last published heading (degrees), for the [`HEADING_DEADBAND_DEG`] dead-band.
    last_heading: Option<f32>,
}

/// The sensor task. Probes all three chips, configures the M10 (NAV-PVT on I²C at the fix rate,
/// NMEA off, TX-Ready on), the BMP581 and the magnetometer, then runs two phases. Boot acquisition
/// holds awake until the first valid fix or [`BOOT_ACQUIRE_TIMEOUT_S`], ignoring the app's power
/// request. The steady state honours the app's [`SensorDemand`], and while riding and stationary it
/// also ticks the compass on its own cadence. The fix poll uses an absolute deadline, so those
/// compass ticks cannot keep resetting it and starve a TX-Ready-less receiver.
#[embassy_executor::task]
pub async fn sensor_task(mut twim: Twim<'static>, mut txready: Input<'static>, link: SensorTaskLink<'static>) {
    info!("sensors: TWIM22 up (SDA P1.04 / SCL P1.03); probing the I²C bus…");

    let baro_addr = probe_bmp581(&mut twim).await;
    let icm_addr = probe_icm20948(&mut twim).await;
    let mut gps_ok = probe_m10(&mut twim).await;

    if let Some(addr) = baro_addr {
        configure_bmp581(&mut twim, addr).await;
    }
    if let Some(addr) = icm_addr {
        configure_icm20948(&mut twim, addr).await;
    }
    if gps_ok {
        set_gnss_running(&mut twim, true).await;
        configure_m10(&mut twim, DEFAULT_INTERVAL_S).await;
    } else {
        warn!("sensors: GPS not answering — retrying during boot acquisition");
    }

    // The AK09916 is read at AK_ADDR through the ICM's bypass, so only the ICM's presence matters
    // at read time.
    let compass_ok = icm_addr.is_some();

    // The warning bundle is published once: at once when the GPS responds, otherwise after the
    // bounded startup window. A receiver still starting must not leave a stale missing-GPS warning.
    let mut presence = SensorPresence { gps: gps_ok, altimeter: baro_addr.is_some(), compass: compass_ok };
    if gps_ok {
        link.dispatch_presence(presence);
    }

    let mut acc = [0u8; ACC_CAP];
    let mut acc_len = 0usize;
    let mut interval_s = DEFAULT_INTERVAL_S;
    let mut st = FixState::default();

    // Phase 1: hold awake until the first valid fix or a bounded timeout, ignoring the app's power
    // request, so the clock is set and the ephemeris warms even on an idle boot.
    info!("sensors: boot acquisition — holding awake for the first fix (≤ {=u64}s)", BOOT_ACQUIRE_TIMEOUT_S);
    let boot_deadline = Instant::now() + Duration::from_secs(BOOT_ACQUIRE_TIMEOUT_S);
    loop {
        if gps_ok {
            wait_data_event(&mut txready, interval_s, &mut st).await;
        } else {
            // An absent receiver cannot supply a TX-Ready edge, so keep its probes at the normal
            // poll cadence.
            Timer::at((Instant::now() + poll_deadline(interval_s)).min(boot_deadline)).await;
            if Instant::now() >= boot_deadline {
                break;
            }
            gps_ok = probe_m10(&mut twim).await;
            if gps_ok {
                set_gnss_running(&mut twim, true).await;
                configure_m10(&mut twim, interval_s).await;
                presence.gps = true;
                link.dispatch_presence(presence);
            }
        }
        if gps_ok && drain_and_publish(&mut twim, &mut acc, &mut acc_len, baro_addr, &mut st, link).await {
            break; // got the boot fix
        }
        if Instant::now() >= boot_deadline {
            warn!(
                "sensors: no boot fix within {=u64}s — proceeding; the clock stays unset until a fix",
                BOOT_ACQUIRE_TIMEOUT_S
            );
            break;
        }
    }
    if !gps_ok {
        error!("sensors: GPS did not answer during boot acquisition — check wiring / power");
        link.dispatch_presence(presence);
    }

    // Power-managed steady state: receiver demand and compass demand are independent.
    let mut power = SensorDemand { gps: GpsPower::Active, compass: false };
    // Send STOP once per idle entry, even if its write fails.
    let mut parked = false;
    // Absolute deadline: compass ticks must not restart it and starve DDC fallback polling.
    let mut next_poll = Instant::now() + poll_deadline(interval_s);
    loop {
        if !power.gnss_running() {
            if !parked {
                set_gnss_running(&mut twim, false).await;
                parked = true;
            }
            // Park GNSS polling while retaining heading updates for an open Peak View.
            let compass_tick = async {
                if power.compass && compass_ok {
                    Timer::after(Duration::from_millis(COMPASS_INTERVAL_MS)).await;
                } else {
                    core::future::pending::<()>().await;
                }
            };
            match select(select(link.wait_power(), link.wait_rate()), compass_tick).await {
                Either::First(Either::First(p)) => power = p,
                Either::First(Either::Second(s)) => {
                    interval_s = s.max(1);
                    continue;
                }
                Either::Second(()) => {
                    read_and_publish_heading(&mut twim, &mut st, link).await;
                    continue;
                }
            }
            if !power.gnss_running() {
                continue; // GNSS remains parked
            }
            parked = false;
            set_gnss_running(&mut twim, true).await;
            // Configuration reads use a separate buffer; discard any partial pre-stop frame.
            acc_len = 0;
            configure_m10(&mut twim, interval_s).await;
            set_power_mode(&mut twim, power.gps).await;
            st.had_fix = false; // acquiring again after GNSS start
            st.stationary = false; // motion state unknown until the first new fix → compass off
            next_poll = Instant::now() + poll_deadline(interval_s);
            continue;
        }

        // The compass timer must not restart the absolute GPS poll deadline.
        let tick_compass = compass_ok && (st.stationary || power.compass);
        let compass_tick = async {
            if tick_compass {
                Timer::after(Duration::from_millis(COMPASS_INTERVAL_MS)).await;
            } else {
                core::future::pending::<()>().await;
            }
        };
        match select(
            select4(txready.wait_for_rising_edge(), Timer::at(next_poll), link.wait_rate(), link.wait_power()),
            compass_tick,
        )
        .await
        {
            Either::First(Either4::First(())) => note_wait_edge(&mut st, true),
            Either::First(Either4::Second(())) => note_wait_edge(&mut st, false),
            Either::First(Either4::Third(new_s)) => {
                interval_s = new_s.max(1);
                info!("sensors: fix interval → {=u16}s (#117); reconfiguring M10", interval_s);
                configure_m10(&mut twim, interval_s).await;
                next_poll = Instant::now() + poll_deadline(interval_s);
                continue;
            }
            Either::First(Either4::Fourth(p)) => {
                if p != power {
                    power = p;
                    if !power.gnss_running() {
                        info!("sensors: position demand ended → requesting GNSS stop");
                    } else {
                        info!("sensors: GPS power → {=str}", power_name(power.gps));
                        set_power_mode(&mut twim, power.gps).await;
                    }
                }
                continue; // Sleep is entered at the top of the loop
            }
            Either::Second(()) => {
                // A compass tick involves no fix, and `next_poll` keeps counting down.
                read_and_publish_heading(&mut twim, &mut st, link).await;
                continue;
            }
        }
        drain_and_publish(&mut twim, &mut acc, &mut acc_len, baro_addr, &mut st, link).await;
        next_poll = Instant::now() + poll_deadline(interval_s);
    }
}

fn poll_deadline(interval_s: u16) -> Duration {
    Duration::from_millis(interval_s as u64 * 1000 + DEADLINE_MARGIN_MS)
}

/// Wait for one DDC data event: a TX-Ready rising edge, or the poll-timeout fallback that makes
/// TX-Ready optional. The steady loop inlines `select4` instead, to also catch rate and power
/// changes.
async fn wait_data_event(txready: &mut Input<'static>, interval_s: u16, st: &mut FixState) {
    let deadline = Duration::from_millis(interval_s as u64 * 1000 + DEADLINE_MARGIN_MS);
    let edge = matches!(select(txready.wait_for_rising_edge(), Timer::after(deadline)).await, Either::First(()));
    note_wait_edge(st, edge);
}

/// Log the TX-Ready and poll-fallback paths the first time each is seen. The fallback is the normal
/// path on a board that does not break TX-Ready out, and points at the P1.05 wiring or the PIO
/// number on one that does.
fn note_wait_edge(st: &mut FixState, txready_edge: bool) {
    if txready_edge {
        if !st.txready_seen {
            info!("sensors: first TX-Ready edge seen — event-driven path live");
            st.txready_seen = true;
        }
    } else if !st.txready_seen && !st.noted_poll_fallback {
        info!("sensors: TX-Ready not seen — using the DDC-poll fallback at the fix rate (expected without a TX-Ready line)");
        st.noted_poll_fallback = true;
    }
}

/// One DDC drain, parse and publish cycle. Publishes the resolved UTC time, which is independent of
/// the position fix so the clock can set during acquisition, and, on a valid fix, a coincident
/// BMP581 reading and the coherent datapoint. Returns whether a valid position fix was published.
async fn drain_and_publish(
    twim: &mut Twim<'static>,
    acc: &mut [u8; ACC_CAP],
    acc_len: &mut usize,
    baro_addr: Option<u8>,
    st: &mut FixState,
    link: SensorTaskLink<'static>,
) -> bool {
    let n = read_ddc(twim, &mut acc[*acc_len..]).await;
    if n == 0 {
        return false;
    }
    *acc_len += n;
    let res = ubx::parse_stream(&acc[..*acc_len]);
    if res.consumed > 0 {
        acc.copy_within(res.consumed..*acc_len, 0);
        *acc_len -= res.consumed;
    } else if *acc_len == ACC_CAP {
        // Full buffer, no complete frame: noise on the bus. Reset rather than wedge.
        warn!("sensors: UBX accumulator full with no frame ({} B) — resetting", *acc_len);
        *acc_len = 0;
    }

    let Some(pvt) = res.nav_pvt else {
        debug!("sensors: {=usize} DDC bytes, no NAV-PVT yet", n);
        return false;
    };

    debug!(
        "NAV-PVT iTOW={=u32} fix={=u8} sats={=u8} hAcc={=u32}mm pDOP={=u16} lat={=i32} lon={=i32}",
        pvt.itow, pvt.fix_type, pvt.num_sv, pvt.hacc_mm, pvt.pdop, pvt.lat, pvt.lon
    );

    // The receiver's UTC time is published before the position-fix gate below, so the clock is set
    // during acquisition, while there is still no usable fix. An unresolved time publishes nothing.
    if let Some(t) = pvt.utc_time() {
        link.dispatch_time(t);
    }

    let Some(fix) = pvt.to_fix() else {
        // No usable fix this epoch, so publish nothing and `poll()` stays `None`.
        if st.had_fix {
            warn!("GPS fix LOST (fixType={=u8} sats={=u8})", pvt.fix_type, pvt.num_sv);
            st.had_fix = false;
        }
        return false;
    };

    // A fallback poll can re-read the same epoch. Skip a repeat so the app never integrates one fix
    // twice; distinct stationary epochs have a new iTOW and still pass.
    if st.last_itow == Some(pvt.itow) {
        return false;
    }
    st.last_itow = Some(pvt.itow);

    // Altitude and temperature are published only on a valid fix, so climb couples to the fix: a
    // GPS outage pauses climb, and no position is logged anyway.
    if let Some(addr) = baro_addr {
        if let Some((pa, c)) = read_bmp581_forced(twim, addr).await {
            let m = bmp581::pa_to_m(pa);
            debug!("BMP581 forced: {=f32} Pa  {=f32} °C  → {=f32} m", pa, c, m);
            link.dispatch_alt(m);
            link.dispatch_temp(c);
        }
    }
    link.dispatch_fix(fix);
    // Motion state for the compass gate: the app uses the magnetometer heading only when the GPS
    // gives no course.
    st.stationary = fix.course.is_none();
    if !st.had_fix {
        info!("GPS FIX acquired: fixType={=u8} sats={=u8} hAcc={=u32}mm", pvt.fix_type, pvt.num_sv, pvt.hacc_mm);
        st.had_fix = true;
    }
    true
}

/// A short defmt-printable name for a [`GpsPower`]; the cross-crate enum has no `Format`.
fn power_name(p: GpsPower) -> &'static str {
    match p {
        GpsPower::Active => "full",
        GpsPower::LowPower => "low (PSMOO)",
        GpsPower::Sleep => "sleep",
    }
}

/// Send a controlled GNSS start/stop without clearing receiver configuration or navigation data.
/// CFG-RST has no ACK. A successful write confirms only that the command was sent.
async fn set_gnss_running(twim: &mut Twim<'static>, running: bool) {
    let mut frame = [0u8; 12];
    let Some(n) = ubx::cfg_gnss_running(&mut frame, running) else { return };
    let action = if running { "start" } else { "stop" };
    if twim.write(M10_ADDR, &frame[..n]).await.is_err() {
        warn!("sensors: CFG-RST GNSS {=str} write failed", action);
    } else {
        info!("sensors: CFG-RST GNSS {=str} sent", action);
    }
}

/// Set the M10's tracking power mode: full power, or on-chip low-power tracking. Verify the
/// `CFG-PM-OPERATEMODE` key and its value semantics on a new board (see
/// [`ubx::KEY_PM_OPERATEMODE`]).
async fn set_power_mode(twim: &mut Twim<'static>, power: GpsPower) {
    let mode = if power == GpsPower::LowPower { 1u8 } else { 0u8 }; // 1 = PSMOO low-power, 0 = full
    valset8(twim, "PM-OPERATEMODE", ubx::KEY_PM_OPERATEMODE, mode).await;
}

/// Probe the BMP581 at its two possible addresses, returning the one that answers. `None` means
/// climb simply does not accumulate.
async fn probe_bmp581(twim: &mut Twim<'static>) -> Option<u8> {
    for addr in [bmp581::ADDR_DEFAULT, bmp581::ADDR_ALT] {
        let mut id = [0u8; 1];
        if twim.write_read(addr, &[bmp581::CHIP_ID], &mut id).await.is_ok() {
            if id[0] == bmp581::CHIP_ID_BMP581 {
                info!("BMP581 found @ {=u8:#04x} (chip_id {=u8:#04x})", addr, id[0]);
            } else {
                // Answered with an unexpected id: use it anyway, but flag the mismatch.
                warn!(
                    "BMP581 @ {=u8:#04x} chip_id {=u8:#04x} (expected {=u8:#04x}) — using anyway",
                    addr,
                    id[0],
                    bmp581::CHIP_ID_BMP581
                );
            }
            return Some(addr);
        }
    }
    error!(
        "BMP581 not found at {=u8:#04x} or {=u8:#04x} (I²C NAK) — altitude/climb disabled",
        bmp581::ADDR_DEFAULT,
        bmp581::ADDR_ALT
    );
    None
}

/// Probe the SAM-M10Q by reading its DDC byte-count register. Absence is reported at the deadline.
async fn probe_m10(twim: &mut Twim<'static>) -> bool {
    let mut cnt = [0u8; 2];
    if twim.write_read(M10_ADDR, &[DDC_COUNT_REG], &mut cnt).await.is_ok() {
        info!("SAM-M10Q alive @ {=u8:#04x} ({=u16} DDC bytes pending)", M10_ADDR, u16::from_be_bytes(cnt));
        true
    } else {
        false
    }
}

/// Write the BMP581 oversampling config. Each reading is then triggered forced in
/// [`read_bmp581_forced`]. On-chip oversampling is the smoothing.
async fn configure_bmp581(twim: &mut Twim<'static>, addr: u8) {
    let osr = twim.write(addr, &[bmp581::OSR_CONFIG, bmp581::OSR_DEFAULT]).await;
    // Enable the data-ready interrupt source, so that its bit shows up in INT_STATUS. It is off
    // after reset, so without this the forced-read poll never sees a completed conversion.
    let src = twim.write(addr, &[bmp581::INT_SOURCE, bmp581::INT_SRC_DRDY_EN]).await;
    if osr.is_err() || src.is_err() {
        warn!("BMP581: config write failed (OSR/INT_SOURCE)");
    } else {
        info!("BMP581 configured (OSR press ×8 / temp ×1, drdy source on, forced-per-fix)");
    }
}

/// Send the M10 VALSET config sequence (RAM layer), confirming each with its UBX-ACK-ACK so a bad
/// key is visible on RTT. Enables NAV-PVT on I²C at `interval_s`, disables NMEA, and arms TX-Ready.
async fn configure_m10(twim: &mut Twim<'static>, interval_s: u16) {
    let meas_ms = interval_s.saturating_mul(1000).max(1000); // CFG-RATE-MEAS is the measurement period
    valset8(twim, "I2COUTPROT-UBX", ubx::KEY_I2COUTPROT_UBX, 1).await;
    valset8(twim, "I2COUTPROT-NMEA", ubx::KEY_I2COUTPROT_NMEA, 0).await;
    valset8(twim, "MSGOUT-NAV_PVT_I2C", ubx::KEY_MSGOUT_NAV_PVT_I2C, 1).await;
    valset16(twim, "RATE-MEAS", ubx::KEY_RATE_MEAS, meas_ms).await;
    valset16(twim, "RATE-NAV", ubx::KEY_RATE_NAV, 1).await;
    valset8(twim, "TXREADY-ENABLED", ubx::KEY_TXREADY_ENABLED, 1).await;
    valset8(twim, "TXREADY-POLARITY", ubx::KEY_TXREADY_POLARITY, 0).await; // active-high
    valset8(twim, "TXREADY-PIN", ubx::KEY_TXREADY_PIN, TXREADY_PIO).await;
    valset16(twim, "TXREADY-THRESHOLD", ubx::KEY_TXREADY_THRESHOLD, TXREADY_THRESHOLD).await;
    valset8(twim, "TXREADY-INTERFACE", ubx::KEY_TXREADY_INTERFACE, 0).await; // 0 = I²C
    info!("SAM-M10Q configured: NAV-PVT @ {=u16}s on I²C, NMEA off, TX-Ready armed", interval_s);
}

/// Build and send one single-key u8 VALSET, and read back its ACK.
async fn valset8(twim: &mut Twim<'static>, name: &str, key: u32, val: u8) {
    let mut frame = [0u8; 20];
    let Some(n) = ubx::valset_u8(&mut frame, key, val) else { return };
    send_valset(twim, name, &frame[..n]).await;
}

/// Build and send one single-key u16 VALSET, and read back its ACK.
async fn valset16(twim: &mut Twim<'static>, name: &str, key: u32, val: u16) {
    let mut frame = [0u8; 21];
    let Some(n) = ubx::valset_u16(&mut frame, key, val) else { return };
    send_valset(twim, name, &frame[..n]).await;
}

/// Write a VALSET frame to the M10, then read back and log its UBX-ACK-ACK or NAK. A missing ACK
/// is logged and not fatal; the receiver may still have applied it.
async fn send_valset(twim: &mut Twim<'static>, name: &str, frame: &[u8]) {
    if twim.write(M10_ADDR, frame).await.is_err() {
        warn!("M10 VALSET {=str}: I²C write failed", name);
        return;
    }
    // Give the receiver a moment to queue the ACK, then drain and scan for it.
    Timer::after_millis(20).await;
    let mut buf = [0u8; 64];
    let n = read_ddc(twim, &mut buf).await;
    match find_valset_ack(&buf[..n]) {
        Some(true) => info!("M10 VALSET {=str}: ACK", name),
        Some(false) => warn!("M10 VALSET {=str}: NAK (bad key/value?)", name),
        None => debug!("M10 VALSET {=str}: no ACK yet (continuing)", name),
    }
}

/// Scan a DDC read for a UBX-ACK answering CFG-VALSET. `None` means no ACK frame is present.
fn find_valset_ack(buf: &[u8]) -> Option<bool> {
    let mut off = 0;
    while let ubx::Scan::Frame { frame, consumed } = ubx::scan_ubx(&buf[off..]) {
        if let Some(ok) = ubx::ack_status(&frame, ubx::CLASS_CFG, ubx::ID_CFG_VALSET) {
            return Some(ok);
        }
        off += consumed;
    }
    None
}

/// Read the DDC byte-count, then drain that many bytes (capped to `out`) from the data register.
/// Returns 0 on a NAK, on an empty or `0xFFFF` count, or when there is no room.
async fn read_ddc(twim: &mut Twim<'static>, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut cnt = [0u8; 2];
    if twim.write_read(M10_ADDR, &[DDC_COUNT_REG], &mut cnt).await.is_err() {
        warn!("sensors: DDC count read failed (I²C)");
        return 0;
    }
    let count = u16::from_be_bytes(cnt);
    if count == 0 || count == 0xFFFF {
        return 0; // no pending data (0xFFFF = the idle/over-read sentinel)
    }
    let n = (count as usize).min(out.len());
    if twim.write_read(M10_ADDR, &[DDC_DATA_REG], &mut out[..n]).await.is_err() {
        warn!("sensors: DDC data read failed (I²C)");
        return 0;
    }
    n
}

/// Trigger one BMP581 forced conversion, wait for it, and read pressure (Pa) and temperature
/// (°C). `None` only on an I²C error.
async fn read_bmp581_forced(twim: &mut Twim<'static>, addr: u8) -> Option<(f32, f32)> {
    // Deep standby is disabled, so the forced conversion starts at once.
    if twim.write(addr, &[bmp581::ODR_CONFIG, bmp581::ODR_FORCED_TRIGGER]).await.is_err() {
        warn!("BMP581: forced-trigger write failed");
        return None;
    }
    // The budget exceeds the worst-case OSR ×8 conversion time, so even if drdy never asserts the
    // data registers hold a valid sample. Read it rather than drop it.
    let mut ready = false;
    for _ in 0..10 {
        Timer::after_millis(3).await;
        let mut st = [0u8; 1];
        if twim.write_read(addr, &[bmp581::INT_STATUS], &mut st).await.is_ok() && st[0] & bmp581::STATUS_DRDY != 0 {
            ready = true;
            break;
        }
    }
    // The six data bytes are contiguous temp(3) then press(3) from TEMP_DATA_XLSB.
    let mut d = [0u8; 6];
    if twim.write_read(addr, &[bmp581::TEMP_DATA_XLSB], &mut d).await.is_err() {
        warn!("BMP581: data read failed");
        return None;
    }
    if !ready {
        debug!("BMP581: drdy didn't assert in budget — read the completed sample anyway");
    }
    let temp_raw = bmp581::raw24_signed(d[0], d[1], d[2]);
    let press_raw = bmp581::raw24_unsigned(d[3], d[4], d[5]);
    Some((bmp581::raw_to_pa(press_raw), bmp581::raw_to_c(temp_raw)))
}

/// Probe the ICM-20948 at its two strap addresses, returning the one whose `WHO_AM_I` reads the
/// expected value. Unlike the baro probe this is strict, because the whole bypass path below
/// assumes the part really is an ICM.
async fn probe_icm20948(twim: &mut Twim<'static>) -> Option<u8> {
    for addr in [icm20948::ADDR_AD0_HIGH, icm20948::ADDR_AD0_LOW] {
        // WHO_AM_I lives in bank 0; select it in case a stray reset left another bank.
        let _ = twim.write(addr, &[icm20948::REG_BANK_SEL, icm20948::BANK_0]).await;
        let mut id = [0u8; 1];
        if twim.write_read(addr, &[icm20948::WHO_AM_I], &mut id).await.is_ok() && id[0] == icm20948::WHO_AM_I_VAL {
            info!("ICM-20948 found @ {=u8:#04x} (who_am_i {=u8:#04x})", addr, id[0]);
            return Some(addr);
        }
    }
    error!(
        "ICM-20948 not found at {=u8:#04x} or {=u8:#04x} (I²C NAK / bad id) — compass heading disabled",
        icm20948::ADDR_AD0_HIGH,
        icm20948::ADDR_AD0_LOW
    );
    None
}

/// Bring the ICM-20948 up for magnetometer-only use: wake it, because reset leaves it asleep, and
/// route its auxiliary I²C bus to the host pins, so the AK09916 answers directly at
/// [`icm20948::AK_ADDR`]. The internal I²C master is already off after reset, so bypass is that one
/// bit. Then soft-reset the AK09916 and confirm it answers through the bypass.
async fn configure_icm20948(twim: &mut Twim<'static>, addr: u8) {
    let wake = twim.write(addr, &[icm20948::PWR_MGMT_1, icm20948::PWR_MGMT_1_WAKE]).await;
    let bypass = twim.write(addr, &[icm20948::INT_PIN_CFG, icm20948::INT_PIN_CFG_BYPASS_EN]).await;
    if wake.is_err() || bypass.is_err() {
        warn!("ICM-20948: config write failed (PWR_MGMT_1 / INT_PIN_CFG) — compass heading may be dead");
        return;
    }
    Timer::after_millis(10).await; // let the bypass mux settle before touching the AK09916
    let _ = twim.write(icm20948::AK_ADDR, &[icm20948::AK_CNTL3, icm20948::AK_CNTL3_SRST]).await;
    Timer::after_millis(10).await;
    let mut wia = [0u8; 1];
    if twim.write_read(icm20948::AK_ADDR, &[icm20948::AK_WIA2], &mut wia).await.is_ok()
        && wia[0] == icm20948::AK_WIA2_VAL
    {
        info!(
            "ICM-20948 magnetometer (AK09916) up via bypass @ {=u8:#04x} (wia2 {=u8:#04x})",
            icm20948::AK_ADDR,
            wia[0]
        );
    } else {
        warn!(
            "ICM-20948: AK09916 not answering through bypass @ {=u8:#04x} (got {=u8:#04x}) — compass heading may be dead",
            icm20948::AK_ADDR,
            wia[0]
        );
    }
}

/// One compass cycle: read the AK09916 heading and publish it, dead-banded by
/// [`HEADING_DEADBAND_DEG`] so that magnetometer noise does not repaint the heading-up map. A read
/// failure or an overflow holds the last heading.
async fn read_and_publish_heading(twim: &mut Twim<'static>, st: &mut FixState, link: SensorTaskLink<'static>) {
    let Some(deg) = read_mag_heading(twim).await else { return };
    let moved = st.last_heading.is_none_or(|h| compass::angle_diff(h, deg) >= HEADING_DEADBAND_DEG);
    if moved {
        st.last_heading = Some(deg);
        debug!("compass: heading {=f32}° (AK09916)", deg);
        link.dispatch_heading(deg);
    }
}

/// Trigger one AK09916 single measurement (single-shot, auto power-down), wait for it, and return
/// the heading in degrees clockwise from north. `None` on an I²C error or a saturated sample. The
/// axis remap and hard-iron offset put the sample in the device frame; [`compass::heading_deg`]
/// then does the chip-agnostic geometry.
async fn read_mag_heading(twim: &mut Twim<'static>) -> Option<f32> {
    if twim.write(icm20948::AK_ADDR, &[icm20948::AK_CNTL2, icm20948::AK_CNTL2_SINGLE]).await.is_err() {
        warn!("compass: AK09916 single-measure trigger failed");
        return None;
    }
    // The budget exceeds the worst-case measurement time, so even if the bit never shows we read
    // the completed sample anyway.
    let mut ready = false;
    for _ in 0..MAG_POLL_TRIES {
        Timer::after_millis(MAG_POLL_MS).await;
        let mut st = [0u8; 1];
        if twim.write_read(icm20948::AK_ADDR, &[icm20948::AK_ST1], &mut st).await.is_ok()
            && st[0] & icm20948::AK_ST1_DRDY != 0
        {
            ready = true;
            break;
        }
    }
    // Burst HXL up to ST2 in one transaction: the read of ST2, the last byte, is what releases the
    // measurement for the next cycle.
    let mut d = [0u8; icm20948::AK_DATA_LEN];
    if twim.write_read(icm20948::AK_ADDR, &[icm20948::AK_HXL], &mut d).await.is_err() {
        warn!("compass: AK09916 data read failed");
        return None;
    }
    if !ready {
        debug!("compass: AK09916 DRDY didn't assert in budget — read the sample anyway");
    }
    if icm20948::overflowed(&d) {
        debug!("compass: AK09916 magnetic overflow — dropping sample");
        return None;
    }
    let (sx, sy, sz) = icm20948::axes_ut(&d)?;
    Some(compass::heading_deg(mag_to_device(sx, sy, sz), DECLINATION_DEG))
}

/// Remap the AK09916's own axes (µT) into the device frame ([`compass::MagSample`]: X forward, Y
/// right, Z down) and remove the [`HARD_IRON_UT`] offset. The remap is identity for now. Verify on
/// glass: rotate the device, confirm the heading tracks and increases clockwise, and fix any axis
/// swap or sign flip here. This is the one board-mounting-specific knob.
fn mag_to_device(sx: f32, sy: f32, sz: f32) -> compass::MagSample {
    compass::MagSample::new(sx - HARD_IRON_UT.0, sy - HARD_IRON_UT.1, sz - HARD_IRON_UT.2)
}
