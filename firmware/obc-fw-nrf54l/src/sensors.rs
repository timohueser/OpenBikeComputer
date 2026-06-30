//! Real GPS (u-blox **SAM-M10Q**) + barometric altimeter (Bosch **BMP581**) on a shared I²C bus
//! (issue #218) — the board-specific transport + the event-driven sensor task.
//!
//! Both chips sit on one **TWIM30** I²C bus on the low-power P0 domain (SDA P0.01 / SCL P0.02); the
//! GPS **TX-Ready** line is the single interrupt (P0.03). The pure decode — UBX NAV-PVT framing,
//! NAV-PVT → [`Fix`](obc_app::Fix), BMP581 raw → metres — lives host-tested in
//! [`obc_platform::ubx`] / [`obc_platform::bmp581`]; this module owns only the concrete `Twim`
//! transactions and the [`sensor_task`] that coalesces a GPS fix + a coincident baro reading into
//! one coherent datapoint and publishes it through [`obc_platform::sensor_link`].
//!
//! ## Event-driven, with a robust fallback (the "no fix" story)
//! The task waits on the **TX-Ready edge** so it does **zero** bus work between fixes. But TX-Ready
//! is the most fragile part of a fresh bring-up (wrong module PIO, polarity, or an unconnected
//! jumper), so the wait also has a **timeout** at roughly the fix interval: if no edge arrives, the
//! task **polls the DDC anyway**, so the GPS still works (at the fix rate) with TX-Ready completely
//! dead — and RTT says so. A NAV-PVT with no fix (`fixType < 3`, cold start / tunnel) publishes
//! **nothing**, so `LocationSource::poll` returns `None` and the camera never teleports; climb
//! simply pauses. Every stage logs over RTT (defmt) so acquisition is watchable live.

use defmt::{debug, error, info, warn};
use embassy_futures::select::{select3, Either3};
use embassy_nrf::gpio::Input;
use embassy_nrf::twim::Twim;
use embassy_time::{Duration, Timer};
use obc_platform::{bmp581, sensor_link, ubx};

/// SAM-M10Q I²C (DDC) slave address.
const M10_ADDR: u8 = 0x42;
/// DDC registers: `0xFD/0xFE` = a 16-bit **big-endian** count of bytes pending in the message
/// buffer; `0xFF` = the data stream (reading it does not auto-increment, so a burst drains the
/// buffer). See the u-blox DDC port description.
const DDC_COUNT_REG: u8 = 0xFD;
const DDC_DATA_REG: u8 = 0xFF;

/// Default GPS fix interval at boot (seconds). The ride loop pushes the persisted #117 setting via
/// [`sensor_link::set_rate`] right after it loads settings, so this only governs the first second.
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

/// The event-driven sensor task (issue #218). Probes both chips, configures the M10 (NAV-PVT on I²C
/// at the fix rate, NMEA off, TX-Ready on) + the BMP581 (oversampling), then loops: wait for a
/// TX-Ready edge (or a fix-rate-interval timeout, or a rate change) → drain the DDC → parse the
/// freshest NAV-PVT → on a **valid fix**, take a coincident BMP581 forced reading and publish the
/// coherent `(fix, altitude, temperature)` datapoint. Spawned once from `main`; never returns.
#[embassy_executor::task]
pub async fn sensor_task(mut twim: Twim<'static>, mut txready: Input<'static>) {
    info!("sensors: TWIM30 up (SDA P0.01 / SCL P0.02); probing the I²C bus…");

    // --- Boot probe: loud RTT so a wiring/power fault is obvious before anything else. ---
    let baro_addr = probe_bmp581(&mut twim).await;
    let gps_ok = probe_m10(&mut twim).await;

    if let Some(addr) = baro_addr {
        configure_bmp581(&mut twim, addr).await;
    }
    if gps_ok {
        configure_m10(&mut twim, DEFAULT_INTERVAL_S).await;
    } else {
        warn!("sensors: GPS not answering — the loop will keep polling so a late-powered module is picked up");
    }

    let mut acc = [0u8; ACC_CAP];
    let mut acc_len = 0usize;
    let mut interval_s = DEFAULT_INTERVAL_S;
    let mut had_fix = false; // for the fix-acquired / fix-lost edge logs
    let mut txready_seen = false; // true once an edge fires → event-driven path live
    let mut warned_no_txready = false; // so the poll-fallback warning fires once, not every cycle
    let mut last_itow: Option<u32> = None; // de-dup a re-read of the same epoch

    loop {
        // Wait for whichever comes first: the TX-Ready edge, the poll timeout (≈ the fix interval,
        // the fallback that makes TX-Ready optional), or a #117 fix-rate change.
        let deadline = Duration::from_millis(interval_s as u64 * 1000 + DEADLINE_MARGIN_MS);
        match select3(txready.wait_for_rising_edge(), Timer::after(deadline), sensor_link::wait_rate()).await {
            Either3::First(()) => {
                if !txready_seen {
                    info!("sensors: first TX-Ready edge seen — event-driven path live");
                    txready_seen = true;
                }
            }
            Either3::Second(()) => {
                if !txready_seen && !warned_no_txready {
                    warn!("sensors: no TX-Ready edge yet — polling DDC at the fix rate (check P0.03 wiring / TX-Ready PIO)");
                    warned_no_txready = true;
                }
            }
            Either3::Third(new_s) => {
                interval_s = new_s.max(1);
                info!("sensors: fix interval → {=u16}s (#117); reconfiguring M10", interval_s);
                configure_m10(&mut twim, interval_s).await;
                continue;
            }
        }

        // Drain the DDC into the accumulator's free tail, then parse the freshest complete NAV-PVT.
        let n = read_ddc(&mut twim, &mut acc[acc_len..]).await;
        if n == 0 {
            continue;
        }
        acc_len += n;
        let res = ubx::parse_stream(&acc[..acc_len]);
        if res.consumed > 0 {
            acc.copy_within(res.consumed..acc_len, 0);
            acc_len -= res.consumed;
        } else if acc_len == ACC_CAP {
            // Full buffer, no complete frame: noise on the bus. Reset rather than wedge.
            warn!("sensors: UBX accumulator full with no frame ({} B) — resetting", acc_len);
            acc_len = 0;
        }

        let Some(pvt) = res.nav_pvt else {
            debug!("sensors: {=usize} DDC bytes, no NAV-PVT yet", n);
            continue;
        };

        // The key acquisition line — watch fixType climb 0→3 and hAcc fall as the receiver locks.
        debug!(
            "NAV-PVT fix={=u8} sats={=u8} hAcc={=u32}mm pDOP={=u16} lat={=i32} lon={=i32}",
            pvt.fix_type, pvt.num_sv, pvt.hacc_mm, pvt.pdop, pvt.lat, pvt.lon
        );

        let Some(fix) = pvt.to_fix() else {
            // No usable fix this epoch (cold start / outage). Publish nothing → poll() stays None.
            if had_fix {
                warn!("GPS fix LOST (fixType={=u8} sats={=u8})", pvt.fix_type, pvt.num_sv);
                had_fix = false;
            }
            continue;
        };

        // De-dup: a fallback poll can re-read the same epoch. Skip a repeat (same iTOW) so the app
        // never integrates one fix twice; distinct stationary epochs (new iTOW) still pass through.
        if last_itow == Some(pvt.itow) {
            continue;
        }
        last_itow = Some(pvt.itow);

        // Valid fix → take a coincident BMP581 forced reading and publish the coherent datapoint.
        // Altitude/temperature are published only on a valid fix, so climb couples to the fix (the
        // documented coherence tradeoff): a GPS outage pauses climb, no position is logged anyway.
        if let Some(addr) = baro_addr {
            if let Some((pa, c)) = read_bmp581_forced(&mut twim, addr).await {
                let m = bmp581::pa_to_m(pa);
                debug!("BMP581 forced: {=f32} Pa  {=f32} °C  → {=f32} m", pa, c, m);
                sensor_link::dispatch_alt(m);
                sensor_link::dispatch_temp(c);
            }
        }
        sensor_link::dispatch_fix(fix);
        if !had_fix {
            info!("GPS FIX acquired: fixType={=u8} sats={=u8} hAcc={=u32}mm", pvt.fix_type, pvt.num_sv, pvt.hacc_mm);
            had_fix = true;
        }
    }
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
