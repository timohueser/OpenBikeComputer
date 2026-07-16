//! Real GPS (u-blox **SAM-M10Q**) + barometric altimeter (Bosch **BMP581**) + electronic compass
//! (the **AK09916** magnetometer inside a TDK **ICM-20948**) on a shared I²C bus — the board-specific
//! transport + the event-driven sensor task.
//!
//! All three chips sit on one **TWIM30** I²C bus on the low-power P0 domain (SDA P0.01 / SCL P0.02);
//! the GPS **TX-Ready** line is the single interrupt (P0.03). The pure decode — UBX NAV-PVT framing,
//! NAV-PVT → [`Fix`](obc_ports::Fix), BMP581 raw → metres, magnetometer axes → heading — lives
//! host-tested in [`obc_sensors::ubx`] / [`obc_sensors::bmp581`] / [`obc_sensors::compass`] /
//! [`obc_sensors::icm20948`]; this module owns only the concrete `Twim` transactions and the
//! [`sensor_task`] that coalesces a GPS fix + a coincident baro + magnetometer reading into one
//! coherent datapoint and publishes it through its [`SensorTaskLink`] into the board's
//! instance-owned [`SensorHub`](obc_platform::sensor_hub::SensorHub) (#808).
//!
//! ## Compass: magnetometer only, via I²C bypass
//! Only the ICM-20948's **3 magnetometer axes** are used — the accel/gyro stay asleep. The AK09916 is
//! reached by putting the ICM in **I²C bypass** (its aux bus tied to the host pins), so it answers
//! directly at `0x0C` as if it were a standalone 3-axis compass. That's deliberate: the shipping
//! board is expected to drop the ICM for a plain magnetometer, and swapping it is then a new chip
//! module like [`obc_sensors::icm20948`] plus new transaction calls here — the heading geometry
//! ([`obc_sensors::compass`]) and the `obc-ports` `CompassSource` seam don't move.
//!
//! Unlike the altimeter (which is logged into each track point and so *must* be fix-coherent), the
//! heading is **never stored** — it only orients a heading-up *map while the rider is stopped*. So it
//! runs on its **own cadence, decoupled from the GPS fix**: ~5 Hz while stationary (lively as you
//! rotate the device by hand, independent of a slow / power-saving fix rate), and silent while moving
//! (the GPS course is the heading then) or idle (the receiver is asleep). See [`sensor_task`].
//!
//! ## Event-driven, with a robust fallback (the "no fix" story)
//! The task waits on the **TX-Ready edge** so it does **zero** bus work between fixes. But TX-Ready
//! is the most fragile part of a fresh bring-up (wrong module PIO, polarity, or an unconnected
//! jumper), so the wait also has a **timeout** at roughly the fix interval: if no edge arrives, the
//! task **polls the DDC anyway**, so the GPS still works (at the fix rate) with TX-Ready completely
//! dead — and RTT says so. A NAV-PVT with no fix (`fixType < 3`, cold start / tunnel) publishes
//! **nothing**, so `LocationSource::poll` returns `None` and the camera never teleports; climb
//! simply pauses. Every stage logs over RTT (defmt) so acquisition is watchable live.
//!
//! ## Power management
//! Continuous tracking is ~20 mA — left on while idle it would flatten the pack in days. So after one
//! **boot fix** (which sets the clock + warms the ephemeris), the task follows the app's
//! [`GpsPower`] request: **deep-sleep** (`RXM-PMREQ` backup, ~µA, zero bus traffic) whenever a ride
//! isn't running, waking on a DDC poke for a fast *warm* fix when one starts; full-power fixes while
//! riding, or the M10's on-chip **low-power** tracking when the `power_saver` toggle is on. The
//! `RXM-PMREQ` / `CFG-PM` encodings live host-tested in [`obc_sensors::ubx`].

use defmt::{debug, error, info, warn};
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_nrf::gpio::Input;
use embassy_nrf::twim::Twim;
use embassy_time::{Duration, Instant, Timer};
use obc_platform::sensor_hub::{GpsPower, SensorPresence, SensorTaskLink};
use obc_sensors::{bmp581, compass, icm20948, ubx};

/// SAM-M10Q I²C (DDC) slave address.
const M10_ADDR: u8 = 0x42;
/// DDC registers: `0xFD/0xFE` = a 16-bit **big-endian** count of bytes pending in the message
/// buffer; `0xFF` = the data stream (reading it does not auto-increment, so a burst drains the
/// buffer). See the u-blox DDC port description.
const DDC_COUNT_REG: u8 = 0xFD;
const DDC_DATA_REG: u8 = 0xFF;

/// Default GPS fix interval at boot (seconds). The ride loop pushes the persisted setting via
/// the sensor hub's rate latch right after it loads settings, so this only governs the first second.
const DEFAULT_INTERVAL_S: u16 = 1;

/// TX-Ready config: the module PIO wired to P0.03, active-high, asserting when ~one NAV-PVT is
/// pending (`THRESHOLD × 8` ≈ 96 B). **VERIFY the PIO number against the SAM-M10Q datasheet on first
/// bring-up** — but note the task does not depend on it: the DDC-poll timeout below keeps fixes
/// flowing if TX-Ready never fires.
const TXREADY_PIO: u8 = 6;
const TXREADY_THRESHOLD: u16 = 12;

/// Extra slop over the fix interval for the DDC-poll timeout, so a full NAV-PVT has finished
/// streaming before a fallback poll reads it.
const DEADLINE_MARGIN_MS: u64 = 300;

/// Persistent UBX byte accumulator: the DDC may hand back a NAV-PVT split across reads, so unparsed
/// tail bytes carry to the next read. 300 B holds ~3 NAV-PVTs (100 B each) plus slack.
const ACC_CAP: usize = 300;

/// Bound on the boot-fix acquisition: the task holds awake at most this long for the first fix —
/// which sets the clock + warms the ephemeris — before dropping into the power-managed steady state,
/// so a boot under cover (no sky) still eventually deep-sleeps when idle.
const BOOT_ACQUIRE_TIMEOUT_S: u64 = 150;

/// Per-board **hard-iron offset** (µT) subtracted from each magnetometer axis before the heading.
/// Hard iron (nearby steel/magnets) shifts the field by a fixed vector; uncalibrated, the compass
/// heading skews. Zero until a calibration routine fills it — **TODO** (rotate-the-device cal). The
/// only essential per-board mag calibration; soft-iron is a later refinement.
const HARD_IRON_UT: (f32, f32, f32) = (0.0, 0.0, 0.0);

/// Magnetic **declination** (degrees east) applied to turn the magnetometer's magnetic heading into
/// the *true*-north heading the GPS course + map use. `0.0` = raw magnetic heading; a future
/// refinement derives the local value from the GPS position (WMM / coarse table) — **TODO**, and a
/// pure add at [`compass::heading_deg`] when it lands.
const DECLINATION_DEG: f32 = 0.0;

/// Conversion budget for one AK09916 single measurement (≈ a few ms typical): poll DRDY this many
/// times at [`MAG_POLL_MS`] before reading the sample anyway (the data registers hold the last
/// conversion regardless). Mirrors the BMP581 forced-read budget.
const MAG_POLL_TRIES: u8 = 10;
const MAG_POLL_MS: u64 = 3;

/// Compass update period (ms) **while stationary** — the heading is read on its own cadence,
/// decoupled from the GPS fix, because (unlike the altimeter) it's never logged: it only orients a
/// heading-up *map while stopped*, and the GPS fix rate is tuned for power/logging, not UI smoothness.
/// ~5 Hz keeps the orientation lively as you rotate the device by hand. Read **only** when stationary
/// and tracking (see [`sensor_task`]); while moving the GPS course is the heading and the compass is
/// silent, and when idle the receiver is asleep — so this never spins the bus needlessly.
const COMPASS_INTERVAL_MS: u64 = 200;

/// Heading dead-band (degrees): publish a new compass heading only once it has moved at least this
/// much from the last published one ([`compass::angle_diff`]). Without it a noisy magnetometer would
/// re-`dispatch_heading` ~5×/s while the device is held still, and each changed heading repaints the
/// heading-up map — burning power for sub-degree jitter. Real turns clear it easily.
const HEADING_DEADBAND_DEG: f32 = 2.0;

/// Per-run state the wait loops + [`drain_and_publish`] thread between cycles: the fix-edge logs,
/// the TX-Ready-vs-poll-fallback notices (each logged once), and the iTOW de-dup.
#[derive(Default)]
struct FixState {
    /// For the fix-acquired / fix-lost edge logs.
    had_fix: bool,
    /// True once a TX-Ready edge fires → the event-driven path is live.
    txready_seen: bool,
    /// So the poll-fallback notice logs once, not every cycle.
    noted_poll_fallback: bool,
    /// De-dup a re-read of the same epoch (a fallback poll can re-read it).
    last_itow: Option<u32>,
    /// Whether the latest valid fix was **stationary** (no GPS course) — the gate for the compass:
    /// the app uses the magnetometer heading only when stopped (`fix.course.is_none()`), so the task
    /// reads it only then. Set on each published fix in [`drain_and_publish`].
    stationary: bool,
    /// Last *published* compass heading (degrees), for the [`HEADING_DEADBAND_DEG`] dead-band.
    last_heading: Option<f32>,
}

/// The sensor task. Probes all three chips, configures the M10 (NAV-PVT on I²C at the fix rate, NMEA
/// off, TX-Ready on) + the BMP581 + the ICM-20948 magnetometer (bypass), then runs two phases:
///
/// 1. **Boot acquisition** — hold awake until the first valid fix (which sets the clock + warms the
///    ephemeris) or [`BOOT_ACQUIRE_TIMEOUT_S`], **ignoring** the app's power request so an idle
///    boot still gets one fix before it can deep-sleep.
/// 2. **Steady state** — honour the app's [`GpsPower`] request: deep-sleep (`RXM-PMREQ` backup, zero
///    bus traffic) when idle; full- (or `power_saver` low-) power fixes while riding. Each waking
///    cycle waits for a TX-Ready edge / poll timeout / rate change / power change, then drains +
///    publishes through [`drain_and_publish`]. While **riding and stationary** it *also* ticks the
///    compass on its own [`COMPASS_INTERVAL_MS`] cadence (the heading isn't logged, so it's decoupled
///    from the fix; while moving the GPS course is the heading and the compass stays silent). The fix
///    poll uses an **absolute** deadline so those compass ticks don't keep resetting it (which would
///    starve a TX-Ready-less receiver's fixes while stopped).
///
/// Spawned once from `main`; never returns.
#[embassy_executor::task]
pub async fn sensor_task(mut twim: Twim<'static>, mut txready: Input<'static>, link: SensorTaskLink<'static>) {
    info!("sensors: TWIM30 up (SDA P0.01 / SCL P0.02); probing the I²C bus…");

    // --- Boot probe: loud RTT so a wiring/power fault is obvious before anything else. ---
    let baro_addr = probe_bmp581(&mut twim).await;
    let icm_addr = probe_icm20948(&mut twim).await;
    let gps_ok = probe_m10(&mut twim).await;

    if let Some(addr) = baro_addr {
        configure_bmp581(&mut twim, addr).await;
    }
    if let Some(addr) = icm_addr {
        configure_icm20948(&mut twim, addr).await;
    }
    if gps_ok {
        configure_m10(&mut twim, DEFAULT_INTERVAL_S).await;
    } else {
        warn!("sensors: GPS not answering — the loop will keep polling so a late-powered module is picked up");
    }

    // Whether the compass is live — the AK09916 magnetometer is read at AK_ADDR through the ICM's
    // bypass, so only its *presence* (a successful ICM probe + config) matters at read time.
    let compass_ok = icm_addr.is_some();

    // Surface the probe result on glass (issue #504): any chip that didn't answer becomes a
    // dismissable warning the ride loop raises. Published once — a missing module is a wiring/power
    // fault, not a transient. (A missing GPS *module* is distinct from "no fix yet".)
    link.dispatch_presence(SensorPresence { gps: gps_ok, altimeter: baro_addr.is_some(), compass: compass_ok });

    let mut acc = [0u8; ACC_CAP];
    let mut acc_len = 0usize;
    let mut interval_s = DEFAULT_INTERVAL_S;
    let mut st = FixState::default();

    // --- Phase 1: boot acquisition. Hold awake until the first valid fix or a bounded timeout,
    // ignoring the app's power request — so the clock gets set and the ephemeris warms even on an idle
    // boot, before the steady state below is allowed to deep-sleep. ---
    info!("sensors: boot acquisition — holding awake for the first fix (≤ {=u64}s)", BOOT_ACQUIRE_TIMEOUT_S);
    let boot_deadline = Instant::now() + Duration::from_secs(BOOT_ACQUIRE_TIMEOUT_S);
    loop {
        wait_data_event(&mut txready, interval_s, &mut st).await;
        if drain_and_publish(&mut twim, &mut acc, &mut acc_len, baro_addr, &mut st, link).await {
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

    // --- Phase 2: power-managed steady state. Honour the app's requested GpsPower — deep-sleep when
    // idle, full / low-power fixes while riding — and keep streaming fixes. ---
    let mut power = GpsPower::Active;
    let mut asleep = false; // so backup is commanded once on entry, not re-sent each parked iteration
                            // Absolute deadline for the next DDC poll fallback. Absolute (not a fresh `Timer::after` each
                            // iteration) so the stationary compass ticks below don't keep restarting it — which would starve
                            // a TX-Ready-less receiver's stationary fixes. Reset only after an actual fix cycle / rate change.
    let mut next_poll = Instant::now() + poll_deadline(interval_s);
    loop {
        if power == GpsPower::Sleep {
            if !asleep {
                enter_backup(&mut twim).await;
                asleep = true;
            }
            // Asleep: zero DDC traffic. Wait only for a power change (or a rate change to apply on
            // the next wake — `CFG-RATE` can't take effect while the receiver is in backup).
            match select(link.wait_power(), link.wait_rate()).await {
                Either::First(p) => power = p,
                Either::Second(s) => {
                    interval_s = s.max(1);
                    continue; // still asleep — re-park; the new rate applies on the next wake
                }
            }
            if power == GpsPower::Sleep {
                continue; // a redundant Sleep request — stay parked
            }
            // Woken → poke the receiver out of backup and re-assert config at the current rate/mode.
            asleep = false;
            wake_receiver(&mut twim).await;
            configure_m10(&mut twim, interval_s).await;
            set_power_mode(&mut twim, power).await;
            st.had_fix = false; // re-acquiring from a warm start
            st.stationary = false; // motion state unknown until the first warm fix → compass off
            next_poll = Instant::now() + poll_deadline(interval_s);
            continue;
        }

        // Active / LowPower: wait for a data event (TX-Ready edge or the absolute poll deadline), a
        // rate change, a power change — or, while stationary, a compass tick. The compass branch
        // is `pending` (never fires) unless the receiver has a compass and the last fix was stopped,
        // so a moving rider does zero magnetometer traffic (the GPS course is the heading then).
        let tick_compass = compass_ok && st.stationary;
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
                    if power == GpsPower::Sleep {
                        info!("sensors: tracking stopped → GPS will deep-sleep");
                    } else {
                        info!("sensors: GPS power → {=str}", power_name(power));
                        set_power_mode(&mut twim, power).await;
                    }
                }
                continue; // Sleep is entered at the top of the loop
            }
            Either::Second(()) => {
                // Stationary compass tick — read + publish the heading (dead-banded). No fix is
                // involved, and `next_poll` is untouched so the GPS poll keeps counting down.
                read_and_publish_heading(&mut twim, &mut st, link).await;
                continue;
            }
        }
        drain_and_publish(&mut twim, &mut acc, &mut acc_len, baro_addr, &mut st, link).await;
        next_poll = Instant::now() + poll_deadline(interval_s);
    }
}

/// The DDC poll-fallback deadline duration for a given fix interval: the interval plus
/// [`DEADLINE_MARGIN_MS`] of slop so a full NAV-PVT has finished streaming before a fallback poll.
fn poll_deadline(interval_s: u16) -> Duration {
    Duration::from_millis(interval_s as u64 * 1000 + DEADLINE_MARGIN_MS)
}

/// Wait for one DDC data event: a TX-Ready rising edge, or the poll-timeout fallback (≈ the fix
/// interval) that makes TX-Ready optional. Logs each path's first occurrence. The boot loop uses
/// this directly; the steady loop instead inlines `select4` to *also* catch rate / power changes.
async fn wait_data_event(txready: &mut Input<'static>, interval_s: u16, st: &mut FixState) {
    let deadline = Duration::from_millis(interval_s as u64 * 1000 + DEADLINE_MARGIN_MS);
    let edge = matches!(select(txready.wait_for_rising_edge(), Timer::after(deadline)).await, Either::First(()));
    note_wait_edge(st, edge);
}

/// Log the TX-Ready / poll-fallback edge the first time each is observed: a TX-Ready edge means the
/// event-driven path is live; the timeout fallback is the normal path on a board that
/// doesn't break TX-Ready out (and points at the P0.03 wiring / PIO on one that does).
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

/// One DDC drain → parse → publish cycle. Drains the receiver's DDC into the
/// accumulator's free tail, parses the freshest complete NAV-PVT, publishes the resolved UTC time
/// (independent of the position fix, so the clock can set during acquisition) and — on a **valid**
/// fix — a coincident BMP581 reading + the coherent `(fix, altitude, temperature)` datapoint, and
/// records whether the fix was stationary (the compass gate). The compass itself is **not** read here
/// — it runs on its own cadence in [`sensor_task`] (the heading isn't logged, so it needn't be fix-
/// coherent). Returns whether a valid position fix was published. Shared by the boot-acquire + steady
/// loops.
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

    // The key acquisition line — watch fixType climb 0→3 and hAcc fall as the receiver locks.
    debug!(
        "NAV-PVT fix={=u8} sats={=u8} hAcc={=u32}mm pDOP={=u16} lat={=i32} lon={=i32}",
        pvt.fix_type, pvt.num_sv, pvt.hacc_mm, pvt.pdop, pvt.lat, pvt.lon
    );

    // Publish the receiver's UTC time the moment it's valid + fully resolved — **before**
    // the position-fix gate below, so the clock is set during acquisition, while there's still no
    // usable fix (a GPS fix always stamps the clock, #641). A `None` (unresolved) publishes nothing.
    if let Some(t) = pvt.utc_time() {
        link.dispatch_time(t);
    }

    let Some(fix) = pvt.to_fix() else {
        // No usable fix this epoch (cold start / outage). Publish nothing → poll() stays None.
        if st.had_fix {
            warn!("GPS fix LOST (fixType={=u8} sats={=u8})", pvt.fix_type, pvt.num_sv);
            st.had_fix = false;
        }
        return false;
    };

    // De-dup: a fallback poll can re-read the same epoch. Skip a repeat (same iTOW) so the app never
    // integrates one fix twice; distinct stationary epochs (new iTOW) still pass through.
    if st.last_itow == Some(pvt.itow) {
        return false;
    }
    st.last_itow = Some(pvt.itow);

    // Valid fix → take a coincident BMP581 forced reading and publish the coherent datapoint.
    // Altitude/temperature are published only on a valid fix, so climb couples to the fix (the
    // documented coherence tradeoff): a GPS outage pauses climb, no position is logged anyway.
    if let Some(addr) = baro_addr {
        if let Some((pa, c)) = read_bmp581_forced(twim, addr).await {
            let m = bmp581::pa_to_m(pa);
            debug!("BMP581 forced: {=f32} Pa  {=f32} °C  → {=f32} m", pa, c, m);
            link.dispatch_alt(m);
            link.dispatch_temp(c);
        }
    }
    link.dispatch_fix(fix);
    // Record motion state for the compass gate: the app uses the magnetometer heading only when the
    // GPS gives no course (stopped), so the task ticks the compass only while this is true.
    st.stationary = fix.course.is_none();
    if !st.had_fix {
        info!("GPS FIX acquired: fixType={=u8} sats={=u8} hAcc={=u32}mm", pvt.fix_type, pvt.num_sv, pvt.hacc_mm);
        st.had_fix = true;
    }
    true
}

/// A short defmt-printable name for a [`GpsPower`] (the cross-crate enum doesn't derive `Format`).
fn power_name(p: GpsPower) -> &'static str {
    match p {
        GpsPower::Active => "full",
        GpsPower::LowPower => "low (PSMOO)",
        GpsPower::Sleep => "sleep",
    }
}

/// Put the M10 into **backup** deep sleep — `RXM-PMREQ`, infinite duration. The
/// receiver keeps its RTC + ephemeris on ~µA and wakes on the next DDC activity, so the restart is a
/// fast *warm* fix. Best-effort: a failed write is logged, not fatal.
async fn enter_backup(twim: &mut Twim<'static>) {
    let mut frame = [0u8; 24];
    let Some(n) = ubx::pmreq_backup(&mut frame) else { return };
    if twim.write(M10_ADDR, &frame[..n]).await.is_err() {
        warn!("sensors: RXM-PMREQ (sleep) write failed — GPS may keep tracking");
    } else {
        info!("sensors: GPS → deep sleep (RXM-PMREQ backup); zero bus traffic until tracking resumes");
    }
}

/// Wake the M10 from backup: any DDC activity wakes it, but the first transaction can be
/// lost while it powers up, so poke the byte-count register a few times with a short settle.
async fn wake_receiver(twim: &mut Twim<'static>) {
    for _ in 0..3 {
        let mut cnt = [0u8; 2];
        let _ = twim.write_read(M10_ADDR, &[DDC_COUNT_REG], &mut cnt).await;
        Timer::after_millis(20).await;
    }
    info!("sensors: GPS woken from backup");
}

/// Set the M10's tracking power mode: full power, or the on-chip low-power tracking when
/// `power_saver` is on. Best-effort VALSET, ACK-logged like the other config keys — **verify the
/// `CFG-PM-OPERATEMODE` key + value semantics on first bring-up** (see [`ubx::KEY_PM_OPERATEMODE`]).
async fn set_power_mode(twim: &mut Twim<'static>, power: GpsPower) {
    let mode = if power == GpsPower::LowPower { 1u8 } else { 0u8 }; // 1 = PSMOO low-power, 0 = full
    valset8(twim, "PM-OPERATEMODE", ubx::KEY_PM_OPERATEMODE, mode).await;
}

/// Probe the BMP581 at its two possible addresses, returning the one that answers (and logging the
/// chip-id), or `None` if neither does (climb then simply doesn't accumulate).
async fn probe_bmp581(twim: &mut Twim<'static>) -> Option<u8> {
    for addr in [bmp581::ADDR_DEFAULT, bmp581::ADDR_ALT] {
        let mut id = [0u8; 1];
        if twim.write_read(addr, &[bmp581::CHIP_ID], &mut id).await.is_ok() {
            if id[0] == bmp581::CHIP_ID_BMP581 {
                info!("BMP581 found @ {=u8:#04x} (chip_id {=u8:#04x})", addr, id[0]);
            } else {
                // Answered but an unexpected id — still use it, just flag the mismatch for bring-up.
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

/// Probe the SAM-M10Q by reading its DDC byte-count register; log whether it answers.
async fn probe_m10(twim: &mut Twim<'static>) -> bool {
    let mut cnt = [0u8; 2];
    if twim.write_read(M10_ADDR, &[DDC_COUNT_REG], &mut cnt).await.is_ok() {
        info!("SAM-M10Q alive @ {=u8:#04x} ({=u16} DDC bytes pending)", M10_ADDR, u16::from_be_bytes(cnt));
        true
    } else {
        error!("SAM-M10Q no ACK on DDC {=u8:#04x} — check Qwiic wiring / 3V3 / V_BCKP", M10_ADDR);
        false
    }
}

/// Write the BMP581 oversampling config (pressure enabled, ×8 / ×1). Each reading is then triggered
/// forced in [`read_bmp581_forced`]. On-chip oversampling is the smoothing; the climb dead-band
/// downstream handles the rest.
async fn configure_bmp581(twim: &mut Twim<'static>, addr: u8) {
    let osr = twim.write(addr, &[bmp581::OSR_CONFIG, bmp581::OSR_DEFAULT]).await;
    // Enable the data-ready interrupt *source* so its bit shows up in INT_STATUS — it's off after
    // reset, so without this the forced-read poll would never see a completed conversion.
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
                                                             // (key, value) sequence. A u16 helper for the two-byte keys, a u8 helper otherwise.
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

/// Build + send one single-key u8 VALSET and read back its ACK (RTT-logging an ACK/NAK/none).
async fn valset8(twim: &mut Twim<'static>, name: &str, key: u32, val: u8) {
    let mut frame = [0u8; 20];
    let Some(n) = ubx::valset_u8(&mut frame, key, val) else { return };
    send_valset(twim, name, &frame[..n]).await;
}

/// Build + send one single-key u16 VALSET and read back its ACK.
async fn valset16(twim: &mut Twim<'static>, name: &str, key: u32, val: u16) {
    let mut frame = [0u8; 21];
    let Some(n) = ubx::valset_u16(&mut frame, key, val) else { return };
    send_valset(twim, name, &frame[..n]).await;
}

/// Write a VALSET frame to the M10 then read back + RTT-log its UBX-ACK-ACK / NAK. Best-effort: a
/// missing ACK is logged, not fatal (the receiver may still have applied it).
async fn send_valset(twim: &mut Twim<'static>, name: &str, frame: &[u8]) {
    if twim.write(M10_ADDR, frame).await.is_err() {
        warn!("M10 VALSET {=str}: I²C write failed", name);
        return;
    }
    // Give the receiver a moment to queue the ACK, then drain + scan for it.
    Timer::after_millis(20).await;
    let mut buf = [0u8; 64];
    let n = read_ddc(twim, &mut buf).await;
    match find_valset_ack(&buf[..n]) {
        Some(true) => info!("M10 VALSET {=str}: ACK", name),
        Some(false) => warn!("M10 VALSET {=str}: NAK (bad key/value?)", name),
        None => debug!("M10 VALSET {=str}: no ACK yet (continuing)", name),
    }
}

/// Scan a DDC read for a UBX-ACK answering CFG-VALSET: `Some(true)` ACK, `Some(false)` NAK, `None`
/// if no ACK frame is present.
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
/// Returns the number of bytes read (0 on a NAK, an empty/`0xFFFF` "no data" count, or no room).
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

/// Trigger one BMP581 forced conversion, wait for it, and read pressure (Pa) + temperature (°C).
/// `None` only on an I²C error.
async fn read_bmp581_forced(twim: &mut Twim<'static>, addr: u8) -> Option<(f32, f32)> {
    // Kick a single forced conversion (deep-standby disabled so it's immediate).
    if twim.write(addr, &[bmp581::ODR_CONFIG, bmp581::ODR_FORCED_TRIGGER]).await.is_err() {
        warn!("BMP581: forced-trigger write failed");
        return None;
    }
    // Wait for the conversion. With the drdy source enabled (configure_bmp581) the data-ready bit
    // gives a fast exit; the ~30 ms budget below comfortably exceeds the worst-case OSR×8 conversion
    // time, so even if drdy never asserts the data registers hold a valid sample — we read it anyway
    // rather than drop it (the bit's behaviour shouldn't gate getting altitude).
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

/// Probe the ICM-20948 at its two strap addresses, returning the one whose `WHO_AM_I` reads `0xEA`
/// (else `None` → compass heading disabled). Unlike the baro probe this is strict: the whole bypass
/// path below assumes it's really an ICM, so a wrong/mismatched id isn't accepted.
async fn probe_icm20948(twim: &mut Twim<'static>) -> Option<u8> {
    for addr in [icm20948::ADDR_AD0_HIGH, icm20948::ADDR_AD0_LOW] {
        // Be defensive about the register bank (WHO_AM_I lives in bank 0) after any stray reset.
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

/// Bring up the ICM-20948 for **magnetometer-only** use: wake it (reset leaves it asleep) and route
/// its auxiliary I²C bus straight to the host pins ([`icm20948::INT_PIN_CFG_BYPASS_EN`]) so the
/// AK09916 answers directly at [`icm20948::AK_ADDR`]. The accel/gyro stay disabled (we read only the
/// 3 mag axes — see [`compass`]) and the internal I²C master is already off after reset, so bypass is
/// just that one bit. Then soft-reset the AK09916 and confirm it's reachable through the bypass.
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

/// One stationary compass cycle: read the AK09916 heading and publish it through
/// [`SensorTaskLink::dispatch_heading`], **dead-banded** by [`HEADING_DEADBAND_DEG`] so magnetometer
/// noise while the device is held still doesn't repaint the heading-up map. Called on the
/// [`COMPASS_INTERVAL_MS`] cadence from [`sensor_task`] while stationary; a read failure / overflow
/// just holds the last heading.
async fn read_and_publish_heading(twim: &mut Twim<'static>, st: &mut FixState, link: SensorTaskLink<'static>) {
    let Some(deg) = read_mag_heading(twim).await else { return };
    let moved = st.last_heading.is_none_or(|h| compass::angle_diff(h, deg) >= HEADING_DEADBAND_DEG);
    if moved {
        st.last_heading = Some(deg);
        debug!("compass: heading {=f32}° (AK09916)", deg);
        link.dispatch_heading(deg);
    }
}

/// Trigger one AK09916 single measurement (single-shot, auto power-down — the magnetometer analogue
/// of the BMP581 forced read), wait for it, and return the heading in degrees CW from north — or
/// `None` on an I²C error or a saturated (overflowed) sample. The board-mounting axis remap +
/// hard-iron offset land the sample in the device frame; [`compass::heading_deg`] then does the
/// chip-agnostic geometry.
async fn read_mag_heading(twim: &mut Twim<'static>) -> Option<f32> {
    if twim.write(icm20948::AK_ADDR, &[icm20948::AK_CNTL2, icm20948::AK_CNTL2_SINGLE]).await.is_err() {
        warn!("compass: AK09916 single-measure trigger failed");
        return None;
    }
    // Wait for DRDY; the budget exceeds the worst-case measurement time, so even if the bit never
    // shows we read the completed sample anyway (like the baro).
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
    // Burst HXL..=ST2 in one transaction — reading ST2 (the last byte) is what releases the
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

/// Remap the AK09916's own `(x, y, z)` axes (µT) into the **device frame** ([`compass::MagSample`]:
/// X forward / Y right / Z down) and remove the [`HARD_IRON_UT`] offset. The remap is **identity**
/// for now — **VERIFY on glass**: rotate the device and confirm the heading tracks and *increases
/// clockwise*; fix any axis swap or sign flip here. This is the one board-mounting-specific knob; it
/// lives next to the transactions (not in the chip-agnostic [`compass`] module) by design.
fn mag_to_device(sx: f32, sy: f32, sz: f32) -> compass::MagSample {
    compass::MagSample::new(sx - HARD_IRON_UT.0, sy - HARD_IRON_UT.1, sz - HARD_IRON_UT.2)
}
