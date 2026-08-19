//! The **protocol-v4 BLE adapter** (`FLAT_Store_Protocol.md` §5.1) — FS7.5-c3a, epic #1256.
//!
//! An adapter "owns record boundaries, pacing, timeouts, drain, and nothing else. It never parses a
//! payload, never mints an identifier, and never originates a frame" (§5). That sentence is the
//! whole design brief for this module, and everything below is one of those four jobs.
//!
//! ## Where the engine is, and why that settles two hard questions at once
//!
//! The engine is **not here**. It lives in [`crate::flat_store::storage_task`], because
//! `obc_link::flat::Store` is synchronous throughout and the card has exactly one writing execution
//! context (#1256's owner ruling). This module reaches it through
//! [`Writer::call`](crate::flat_store::Writer::call), one record at a time.
//!
//! That placement is also what makes the two obligations §5 puts on a binding cheap rather than
//! intricate:
//!
//! - **Cross-channel ordering.** §5 requires that "a control frame reaches the engine before any
//!   stream frame bearing the same `RequestId`", and an adapter that cannot order the two must hold
//!   one stream frame and withhold link credit rather than drop or deliver it. Here both channels
//!   funnel through **one** task ([`serve_objects`]), so ordering is not negotiated between two
//!   futures — it is the order that one loop takes its two inputs, and the loop takes the control
//!   input first. When a stream SDU arrives with a control record already pending, the SDU stays in
//!   [`STREAM_RX`] and **no further SDU is read** until the control record has been served: that is
//!   the hold and the credit withholding, spelled as a loop rather than as a buffer pool.
//! - **One reply slot.** `Writer::call`'s contract is one slot per *concurrently live* call. One
//!   driver means one live call, so BLE spends exactly one [`Reply`] — [`ENGINE_REPLY`] — and the
//!   two-waiters-on-one-`Signal` failure that contract exists to prevent cannot be constructed here.
//!
//! ## What a client must do, and the one c3a requirement worth ratifying
//!
//! §5.1's shape is unchanged: read `protocolVersion` (now two bytes, `4`), read `psm`, enable
//! indications on `objectControl`, then write control frames and open the L2CAP CoC for stream
//! records.
//!
//! **In c3a the CoC must be open before a control frame is answered.** The engine driver owns the
//! channel, so until one is accepted there is no loop pumping the engine. A control-only client —
//! one that wants nothing but `LIST`, `STATUS` or `REMOVE` — therefore still opens the channel. The
//! specification does not forbid that (the stream channel is part of the binding either way), but it
//! does not require it either, and it is a client-visible fact rather than an implementation detail,
//! so it is called out here and in the PR body rather than left to be discovered. Lifting it means
//! servicing control records while no channel exists, which needs the driver split from the channel
//! owner; that is a named follow-up, not a silent gap.
//!
//! ## What the buffers are, and why they are `.bss`
//!
//! Three, all `static` for the #677 reason every buffer on this board is: a local in an async body
//! becomes a permanent slot in the generated poll frame, allocated at entry on every poll. They are
//! also what makes a record `'static`, which is what the queue needs — a request outlives the
//! statement that sent it, so a borrowed ATT write or a borrowed SDU could not cross it.

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration};
use nrf_sdc::{self as sdc};
use obc_link::flat::{Channel, Reaction};
use trouble_host::prelude::*;

use crate::flat_store::{Outcome, Reply, Request, Writer, PREFERRED_CONTROL_CEILING, PREFERRED_STREAM_CEILING};

use super::gatt::Server;

/// One indication, bounded. §5.1 makes a response a *confirmed* indication, and trouble-host blocks
/// until the peer's `HandleValueConfirmation` for up to the 30 s ATT transaction timeout — which is
/// far past anything this device should wait on. The bound is the same structural backstop the
/// `status` notify has had since #277: a peer that stops confirming must not park this task past the
/// link's supervision timeout.
const INDICATE_TIMEOUT: Duration = Duration::from_secs(5);

// ══════════════════════════ the buffers ══════════════════════════

/// The reaction buffer, and the ceiling cap both channels are pinned under.
///
/// 256 rather than 245 so that the two §5.1 ceilings — a 244-byte control record at the preferred
/// 247-byte ATT MTU, and a CoC SDU of the packet pool's MTU − 6 — both fit with the slack a link
/// that negotiates *upward* would need. [`ceilings_for`] clamps to it either way, so the buffer is
/// the authority and not merely the usual case.
const OUT_LEN: usize = 256;

/// Where a reaction's bytes land. Lent to the engine for the length of one call and taken back with
/// the answer; see [`Lane`].
static mut OUT: [u8; OUT_LEN] = [0; OUT_LEN];

/// One control record, copied out of the ATT write so it can cross the queue as `'static`.
static mut CONTROL_RX: [u8; OUT_LEN] = [0; OUT_LEN];

/// One stream record. Sized to the packet pool's MTU because that is the largest SDU the CoC can
/// hand back, and **this is the frame §5's hold is about**: while it is occupied and a control
/// record is pending, no further SDU is read.
static mut STREAM_RX: [u8; DefaultPacketPool::MTU] = [0; DefaultPacketPool::MTU];

/// BLE's one engine reply slot. One driver, one live call — see the module docs.
static ENGINE_REPLY: Reply = Signal::new();

/// A control record the GATT task has staged, as its length in [`CONTROL_RX`].
static CONTROL_IN: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The driver has taken [`CONTROL_RX`] and the GATT task may stage another.
///
/// ATT Write Requests from one peer are already serialized by their responses, so this closes a
/// window rather than a gap — but the window is real: the GATT task accepts the write as soon as it
/// has staged the record, so without this handshake a peer that got its Write Response could stage a
/// second record over a first the driver had not yet copied.
static CONTROL_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The adapter's resident cost, for the budget table in `main.rs`.
pub(crate) const RESIDENT_BYTES: usize = OUT_LEN + OUT_LEN + DefaultPacketPool::MTU;

// ══════════════════════════ the control channel ══════════════════════════

/// **Stage one `objectControl` write for the driver** — called from the GATT event pump, inside the
/// write's `with_data` closure.
///
/// Returns `false` when the record cannot be staged, which the caller turns into an ATT error rather
/// than a silence: a frame longer than this link can carry is `invalidFrame` territory, and a driver
/// that is not running (no CoC yet — see the module docs) would otherwise leave the write
/// unanswered forever.
///
/// It does **not** parse. The length check is a buffer bound, not a protocol opinion; every verdict
/// about these bytes is the engine's.
pub(crate) fn stage_control(record: &[u8]) -> bool {
    if record.is_empty() || record.len() > OUT_LEN {
        warn!("ble: [v4] objectControl write is {} B — outside this link's record bound", record.len());
        return false;
    }
    if CONTROL_IN.signaled() {
        warn!("ble: [v4] a control record is still un-taken — refusing the write rather than overwriting it");
        return false;
    }
    // SAFETY: the driver has released `CONTROL_RX` — either it never held it, or it signalled
    // `CONTROL_TAKEN` after copying — and `CONTROL_IN` is clear, which is the flag that says so.
    // Both this and the driver run as cooperative futures on the one thread-mode executor, and
    // neither holds the buffer across an `await`.
    unsafe {
        let staging = &mut *core::ptr::addr_of_mut!(CONTROL_RX);
        staging[..record.len()].copy_from_slice(record);
    }
    CONTROL_TAKEN.reset();
    CONTROL_IN.signal(record.len());
    true
}

/// Wait until the driver has taken the staged record, so the next write cannot race it.
///
/// Bounded by the caller: this is awaited from the GATT pump after the ATT response is queued, and a
/// driver that never runs would otherwise hold the pump. See [`stage_control`]'s refusal path for
/// the other half.
pub(crate) async fn control_taken() {
    let _ = with_timeout(INDICATE_TIMEOUT, CONTROL_TAKEN.wait()).await;
}

// ══════════════════════════ the lane ══════════════════════════

/// The adapter's half of one round trip to the engine: the reply slot, and the buffer it lends.
///
/// The buffer is `Option` because it is genuinely *lent* — it crosses the queue inside the request
/// and comes back inside the answer. A `None` here means a previous call's future was dropped
/// between the send and the answer (a link that went away mid-record), and the lane is dead until
/// the next connection rebuilds it. That is why [`Lane::new`] is per connection and not per boot.
struct Lane {
    out: Option<&'static mut [u8]>,
}

impl Lane {
    /// Take the reaction buffer for this connection.
    ///
    /// # Safety
    /// One lane exists at a time: [`serve_objects`] is the sole caller and there is one BLE
    /// connection. A previous connection's driver is dropped before this runs, and the storage task
    /// holds the borrow only inside its synchronous `serve` — never across the `signal` — so no
    /// live reference to [`OUT`] survives into this one.
    unsafe fn new() -> Self {
        Lane { out: Some(&mut *core::ptr::addr_of_mut!(OUT)) }
    }

    /// Hand one request to the engine and take the buffer back with its answer.
    ///
    /// **Never dropped mid-call by this module.** Every call site awaits it directly rather than
    /// inside a `select`, which is what keeps the lend/return cycle closed; the `None` arm is the
    /// honest report of the one case that can still break it — the whole driver being dropped.
    async fn call(&mut self, writer: &Writer, make: impl FnOnce(&'static mut [u8]) -> Request) -> Option<Reaction> {
        let out = self.out.take()?;
        match writer.call(make(out), &ENGINE_REPLY).await {
            Ok(Outcome::Reacted { reaction, out }) => {
                self.out = Some(out);
                Some(reaction)
            }
            // `serve` answers these three requests with `Reacted` and nothing else, so the buffer is
            // gone only if that stopped being true. Report rather than panic: this is a radio task.
            other => {
                warn!("ble: [v4] the engine answered a record with the wrong shape — lane closed ({})", other.is_ok());
                None
            }
        }
    }

    /// The bytes a [`Reaction::Send`] named.
    fn sent(&self, len: usize) -> &[u8] {
        match &self.out {
            Some(out) => &out[..len.min(out.len())],
            None => &[],
        }
    }
}

// ══════════════════════════ the driver ══════════════════════════

/// **The engine driver: one loop, both channels, one reply slot.**
///
/// Replaces the v1 `serve_coc`. The CoC no longer carries an unframed object stream — it carries
/// §3.8 stream records, one per SDU, and the control channel is the `objectControl` characteristic
/// rather than `transferControl` plus a status notify.
pub(crate) async fn serve_objects(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ! {
    let Some(writer) = crate::flat_store::writer() else {
        // Unreachable since c3a spawns the storage task on every card, and reported rather than
        // unwrapped because a radio task must never be why the board panics.
        warn!("ble: [v4] the store's write half is not armed — object service is down this boot");
        core::future::pending().await
    };
    // SAFETY: see `Lane::new`. One connection, one driver, and the previous one is gone.
    let mut lane = unsafe { Lane::new() };
    let listener = L2capChannel::listen(stack, conn.raw());
    loop {
        let mut ch = match listener.accept(&L2capChannelConfig::default()).await {
            Ok(ch) => ch,
            Err(e) => {
                warn!("ble: [v4] accept failed: {:?}", defmt::Debug2Format(&e));
                embassy_time::Timer::after_millis(200).await;
                continue;
            }
        };
        // The CoC requires an encrypted link. In practice a peer cannot reach here unencrypted —
        // `psm` and `objectControl` are both `authenticated` — but one that guessed the SPSM must
        // still be turned away.
        if !matches!(conn.raw().security_level(), Ok(level) if level.encrypted()) {
            warn!("ble: [v4] channel opened on an unencrypted link — refusing (S0 §8)");
            ch.disconnect();
            continue;
        }
        let (control, stream) = ceilings_for(conn, &ch);
        info!("ble: [v4] channel up — control {} B, stream {} B (coc mtu {})", control, stream, ch.mtu());
        // §5.1: a link below the protocol floor cannot carry this protocol, and the adapter refuses
        // the connection rather than truncating. `LinkUp` is where that verdict is reached, because
        // the floor is the engine's constant and not this module's.
        match writer.call(Request::LinkUp { control, stream }, &ENGINE_REPLY).await {
            Ok(_) => {}
            Err(_) => {
                warn!("ble: [v4] this link is below the protocol floor — closing the channel");
                ch.disconnect();
                continue;
            }
        }
        let outcome = serve_channel(&writer, &mut lane, stack, server, conn, &mut ch).await;
        // §3.8's third form of cancel. Not optional: the live transfer's allocation or hold is
        // released here or not at all, and a dropped one is a row the card keeps until the next
        // mount.
        let _ = writer.call(Request::LinkLost, &ENGINE_REPLY).await;
        info!("ble: [v4] channel down ({}) — engine released, re-accepting", outcome);
    }
}

/// Serve one accepted channel until it drops. Returns a short reason for the log.
async fn serve_channel(
    writer: &Writer,
    lane: &mut Lane,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
) -> &'static str {
    loop {
        // **The cross-channel ordering guarantee, and it is this line.** A control record that is
        // already pending is served before any SDU is taken off the channel, so a `PUT`'s admission
        // always reaches the engine ahead of the stream frames that belong to it (§3.6, §5).
        if let Some(len) = CONTROL_IN.try_take() {
            CONTROL_TAKEN.signal(());
            if !drive_control(writer, lane, stack, server, conn, ch, len).await {
                return "control";
            }
            continue;
        }
        // SAFETY: the driver is the sole user of `STREAM_RX`, and this is the driver.
        let rx = unsafe { &mut *core::ptr::addr_of_mut!(STREAM_RX) };
        let received = match select(CONTROL_IN.wait(), ch.receive(stack, rx)).await {
            Either::First(len) => {
                CONTROL_TAKEN.signal(());
                if !drive_control(writer, lane, stack, server, conn, ch, len).await {
                    return "control";
                }
                continue;
            }
            Either::Second(Ok(n)) if n > 0 => n,
            Either::Second(Ok(_)) => continue,
            Either::Second(Err(_)) => return "channel",
        };
        // §5's hold: this SDU is in `STREAM_RX` and we read no further one. If a control record
        // arrived while it was in flight, it was written first and goes to the engine first — after
        // which this frame is delivered, not dropped.
        if let Some(len) = CONTROL_IN.try_take() {
            CONTROL_TAKEN.signal(());
            if !drive_control(writer, lane, stack, server, conn, ch, len).await {
                return "control";
            }
        }
        // SAFETY: as above — one driver, and nothing else names this buffer.
        let record: &'static [u8] =
            unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(STREAM_RX).cast::<u8>(), received) };
        let Some(reaction) = lane.call(writer, |out| Request::Stream { record, out }).await else {
            return "lane";
        };
        if !pump(writer, lane, stack, server, conn, ch, reaction).await {
            return "send";
        }
    }
}

/// Hand one staged control record to the engine and drive whatever it answers.
#[allow(clippy::too_many_arguments)]
async fn drive_control(
    writer: &Writer,
    lane: &mut Lane,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    len: usize,
) -> bool {
    // SAFETY: `CONTROL_IN` was taken, so the GATT task is not writing this buffer, and
    // `CONTROL_TAKEN` has been signalled so it may stage the next one only after this copy is
    // consumed by the engine — which happens inside the call below, synchronously in the storage
    // task, before the answer this awaits.
    let record: &'static [u8] =
        unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(CONTROL_RX).cast::<u8>(), len) };
    let Some(reaction) = lane.call(writer, |out| Request::Control { record, out }).await else {
        return false;
    };
    pump(writer, lane, stack, server, conn, ch, reaction).await
}

/// The driver loop of `obc_link::flat::engine`: send what the reaction names, then pump until the
/// engine goes quiet. A driver that stops pumping stalls a download, so this is not optional.
#[allow(clippy::too_many_arguments)]
async fn pump(
    writer: &Writer,
    lane: &mut Lane,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    first: Reaction,
) -> bool {
    let mut reaction = first;
    loop {
        match reaction {
            Reaction::Idle => return true,
            Reaction::Close(_) => {
                // §3.1's unanswerable record: emit nothing and close that record stream.
                info!("ble: [v4] unanswerable record — closing the channel");
                ch.disconnect();
                return false;
            }
            Reaction::Send { channel, len } => {
                let ok = match channel {
                    Channel::Control => {
                        // §5.1: one confirmed indication carries the response.
                        let bytes = lane.sent(len);
                        match with_timeout(INDICATE_TIMEOUT, server.obc.object_control.indicate_raw(conn, bytes, false))
                            .await
                        {
                            Ok(Ok(())) => true,
                            Ok(Err(e)) => {
                                warn!("ble: [v4] indicate failed: {:?}", defmt::Debug2Format(&e));
                                false
                            }
                            Err(_) => {
                                warn!("ble: [v4] indicate timed out — abandoning");
                                false
                            }
                        }
                    }
                    Channel::Stream => ch.send(stack, lane.sent(len)).await.is_ok(),
                };
                if !ok {
                    return false;
                }
            }
            Reaction::SendAndReboot { .. } => {
                // §4 step 4. `ARM` is refused by this build's policy (`flat_store::BoardPolicy`), so
                // the engine never reaches this reaction; it is answered rather than ignored so that
                // the slice which fills the policy finds a driver that already handles it.
                warn!("ble: [v4] the engine asked for a reboot, which this build cannot arm — dropping the link");
                return false;
            }
        }
        let Some(next) = lane.call(writer, |out| Request::Pump { out }).await else {
            return false;
        };
        reaction = next;
    }
}

/// What this link can actually carry, clamped to what [`OUT`] can hold.
///
/// The clamp is load-bearing rather than defensive: the engine frames a `LIST` page and a stream
/// record against its ceilings, and a link that negotiated *upward* of this buffer would otherwise
/// have it framing into bytes that are not there.
fn ceilings_for(
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    ch: &L2capChannel<'_, DefaultPacketPool>,
) -> (usize, usize) {
    // §5.1: "The control ceiling is `ATT_MTU - 3`".
    let att = usize::from(conn.raw().att_mtu()).saturating_sub(3);
    let control = if att == 0 { PREFERRED_CONTROL_CEILING } else { att }.min(OUT_LEN);
    let stream = usize::from(ch.mtu()).min(OUT_LEN).max(1);
    let _ = PREFERRED_STREAM_CEILING;
    (control, stream)
}
