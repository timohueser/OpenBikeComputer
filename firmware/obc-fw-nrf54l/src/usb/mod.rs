//! The **USB device plane** (issues #889, #1420): the nRF54LM20's USBHS binding v5 carrying §3
//! protocol-v4 frames, so a
//! browser (WebUSB) or the desktop app can push a map, a route or a firmware image straight to a
//! plugged-in device.
//!
//! ## Why this is a transport, not a protocol
//!
//! `FLAT_Store_Protocol.md` §5 puts it plainly: "an adapter owns record boundaries, pacing,
//! timeouts, drain, and nothing else." The frame bytes are identical on both links, so nothing here
//! decides what a message *means* — that is the engine's, and the engine lives beside the card in
//! [`crate::flat_store::storage_task`].
//!
//! What USB adds is one thing and it is small: §5.2's `record_length u32` plus alignment padding
//! around every frame, because a bulk endpoint is a byte pipe and BLE's channels are already
//! message-shaped. See [`records`]. The one thing that is *not* a §3 frame — the device information
//! a host reads before it exchanges a record — is an EP0 vendor request, which is USB's own place
//! for identity. See [`device_info`] and §5.2.1.
//!
//! ## Endpoint layout (the host contract)
//!
//! One vendor-specific interface (class `0xFF`, `bInterfaceProtocol = 5`), four bulk endpoints,
//! allocated in this order so the host's "lowest IN/OUT pair is control, the next is stream" rule
//! (`builder/app/src/lib/usb/webusb.ts::discoverLayout`) reads them correctly:
//!
//! | Endpoint | Direction | Carries |
//! | :-- | :-- | :-- |
//! | 0x81 / 0x01 | IN / OUT | §3 control records, each `record_length u32` + frame + padding |
//! | 0x82 / 0x02 | IN / OUT | §3.8 stream records, same framing |
//!
//! The USBHS is a **high-speed** core (`PhyType::InternalHighSpeed`), so every bulk endpoint is
//! 512 bytes by USB rule; there is no full-speed fallback to size for. Packet boundaries carry no
//! protocol meaning (§5.2) — a record may span them, which is what lets an 8 KiB stream payload exist
//! on a 512-byte endpoint at all.
//!
//! ## VBUS is a hard gate, not a convenience (#936)
//!
//! **The device must boot and ride with nothing plugged into J3** — that is the overwhelmingly
//! common case, and USB is the exception. Nothing in this module may touch the USBHS core, or any
//! future that reads it, unless a cable is *actually* present; see [`vbus_present`] for the
//! mechanism and the on-glass failure it fixes.
//!
//! ## Pins, clocks, interrupts
//!
//! **Zero GPIO cost**: D+/D-/VBUS/TXRTUNE are dedicated USBHS pins, so nothing in the board's pin
//! plan moves. The driver needs AHB >= 30 MHz (asserted in its `calculate_trdt`); the board runs
//! 128 MHz. It starts the **XO24M** oscillator itself while enabled. The board binding gives it
//! `USBHS` and `VREGUSB`; neither vector overlaps MPSL or the SDC (MPSL takes `RADIO_0`,
//! `TIMER10`, `GRTC_3`, `CLOCK_POWER`, `SWI00`; the board's high-priority input executor is on
//! `SWI01`).
//!
//! ## Concurrency with the ride loop — the reason MSC was rejected
//!
//! The app keeps running while a transfer lands. Every card write goes through the one storage task
//! and takes the card per command rather than per commit, so the ride loop's map render interleaves
//! between records exactly as it does for BLE. Mass Storage would instead have handed the host raw
//! block access and forced the firmware to release the card entirely; firmware-mediated writes
//! compose with the existing arbitration instead.

pub(crate) mod device_info;
pub(crate) mod records;
pub(crate) mod v4;

use core::future::{poll_fn, Future};
use core::mem::MaybeUninit;
use core::task::Poll;

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_nrf::usb::vbus_detect::{HardwareVbusDetect, VbusDetect};
use embassy_nrf::usb::{self as nrf_usb, Driver as UsbhsDriver};
use embassy_nrf::{interrupt, pac, peripherals, Peri};
use embassy_sync::waitqueue::AtomicWaker;
use embassy_usb::msos::{self, windows_version};
use embassy_usb::{Builder, Config, UsbDevice};

use crate::ble::set_usb_radio_inhibited;
use crate::init_static;
use crate::link::identity;
use obc_link::flat::USB_BINDING_MAJOR;

use device_info::{DeviceInfoHandler, MAX_DEVICE_INFO};

// ============================ Identity on the wire ============================

/// **Development** USB vendor / product id.
///
/// `0x1209` is [pid.codes](https://pid.codes), the community vendor id for open-source hardware,
/// and `0x0001` is its documented **prototype / testing** pair — the id whose entire meaning is
/// "not allocated yet". That is deliberate: shipping a made-up id under someone else's VID is
/// worse than shipping one that admits it is provisional, and any other prototype on the same
/// machine matching this filter is a bring-up annoyance, not a correctness problem.
///
/// Allocating a real id is an **owner action, not a firmware change** — see the PR body. When it
/// lands, exactly two constants move: [`PRODUCT_ID`] here and `OBC_USB_FILTERS` in
/// `builder/app/src/lib/usb/webusb.ts`.
const VENDOR_ID: u16 = 0x1209;
const PRODUCT_ID: u16 = 0x0001;

/// The `iManufacturer` / `iProduct` strings. `iSerialNumber` is the FICR device id (see
/// [`identity::serial_string`]) — it is what makes two boards distinguishable in the browser's
/// device chooser and what lets a host remember *which* device it was granted.
const MANUFACTURER: &str = "OpenBikeComputer";
const PRODUCT: &str = "OpenBikeComputer";

/// Every bulk endpoint on a high-speed device is 512 bytes — the USB 2.0 rule, not a choice.
///
/// It is **not** a frame ceiling any more, and that is the v4 cutover in one constant: §5.2's
/// records span packets, so a record's length is a number in front of it rather than the size of the
/// transfer that carried it. What this still bounds is one `write` call on an IN endpoint (one call
/// is one packet on this driver) and the arming of the control OUT endpoint.
const MAX_PACKET: u16 = 512;

/// **How many max packets the bulk OUT endpoint arms at once** — the upload throughput dial
/// (#1173), and the reason `embassy-usb-synopsys-otg` is vendored (`vendor/…/VENDORING.md`).
///
/// # What the number does
///
/// Stock, the driver arms exactly one packet and re-arms it only after this firmware's task has
/// been scheduled, copied the packet out and cleared NAK. The endpoint NAKs for that whole round
/// trip — ISR → waker → executor scan → poll → copy → `CNAK` — once per 512 B. Measured on glass
/// 2026-08-07: ~342 µs per packet, of which ~240 µs was that serialisation, capping map uploads at
/// 1416–1459 kB/s on *both* hosts (WebUSB and desktop), which is what said the ceiling was here and
/// not on the wire. Arming N packets amortises the round trip over N: the core keeps absorbing the
/// stream while the CPU advances the transfer and writes the card, and the reader picks up everything that
/// accumulated in one go.
///
/// # The number, and how to sweep it
///
/// **16, for 8 KiB bursts.** One burst carries a complete USB stream record, so the endpoint is
/// re-armed once per record rather than halfway through it. Together with binding v5's aligned
/// record spans this measured 6,528 kB/s for an 850,824,480-byte real map and 7,112 kB/s for the
/// sparse acceptance fixture on the LM20 board (2026-08-20). The RAM cost stays strictly linear at
/// **two** buffers of `N × 512 B` (the adapter reassembly tail and the driver's per-endpoint staging
/// area inside [`EP_BUFFER`]), and is itemized in `resource_baseline.json`. The setting must also
/// fit the core's RX FIFO: 3040 words total, of which a bursting endpoint takes `N × 129`, so N=16
/// (2064 words) is the last rung that fits beside everything else.
///
/// To sweep it: change this one line, re-pin `compile_time_allocations.usb_named` +
/// `resident_ram_max`/`measured_resident` (they move by `2 × 512 × ΔN`) and `residual_stack_min`
/// (down by the same) in `firmware/tools/resource_baseline.json` on **both** profiles, then read the
/// `~{} kB/s` line the v4 adapter prints at the end of every transfer over RTT. Do not
/// re-pin from a host progress estimate: the device-side interval includes the binding, engine and
/// card path this dial changes.
const BULK_OUT_BURST_PACKETS: u16 = 16;

/// One burst: what [`BULK_BUF`] holds and what the bulk OUT endpoint arms.
const BULK_BURST_LEN: usize = BULK_OUT_BURST_PACKETS as usize * MAX_PACKET as usize;

/// Payload bytes combined before the flat store sees one card write. This is eight protocol-v4
/// stream records and one 128-block CMD25, matching the measured efficient width of the sEMMC
/// path. The bytes are an arm of the existing scratch arena, not additional resident RAM.
pub(crate) const STAGE_HALF_LEN: usize = 64 * 1024;
pub(crate) const STAGE_LEN: usize = 2 * STAGE_HALF_LEN;

// The USB task asks; the ride loop, which exclusively switches scratch-arena owners, grants. Level
// bits are the truth and signals only wake the other side, so an edge cannot be lost.
static STAGE_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static STAGE_GRANTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static STAGE_WAKE: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();
static STAGE_EDGE: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();

pub(crate) fn stage_requested() -> bool {
    STAGE_REQ.load(core::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn set_stage_granted(granted: bool) {
    STAGE_GRANTED.store(granted, core::sync::atomic::Ordering::Relaxed);
    STAGE_EDGE.signal(());
}

pub(crate) async fn wait_stage_request() {
    STAGE_WAKE.wait().await
}

/// Ask for the arena arm once at the beginning of a map stream. A missed grant degrades to the
/// ordinary 512-byte engine stage; it never blocks the transfer indefinitely.
pub(crate) async fn request_stage() -> bool {
    STAGE_REQ.store(true, core::sync::atomic::Ordering::Relaxed);
    STAGE_EDGE.reset();
    STAGE_WAKE.signal(());
    let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_secs(1);
    while !STAGE_GRANTED.load(core::sync::atomic::Ordering::Relaxed) {
        if embassy_time::with_deadline(deadline, STAGE_EDGE.wait()).await.is_err() {
            cancel_stage_request();
            warn!("usb: [v4] no upload staging arm granted — using narrow card writes");
            return false;
        }
    }
    true
}

/// Withdraw a request that never received a grant. Once a grant exists, only v4's joined-stage
/// typestate path may clear it; this seam therefore cannot release DMA-owned arena bytes.
fn cancel_stage_request() {
    STAGE_REQ.store(false, core::sync::atomic::Ordering::Relaxed);
    STAGE_WAKE.signal(());
}

/// The endpoint **index** the bulk OUT pipe lands on, which is what the driver's burst mask is
/// keyed on ([`Config::out_burst_endpoints`](nrf_usb::Config)).
///
/// It is 2 because of the allocation order in [`build_plane`] — EP0 is the control pipe, EP1 is the
/// control-frame pair, EP2 is the bulk pair — and that order is already the host's wire contract, so
/// this is not a second fragile assumption but the same one. [`build_plane`] asserts it against the
/// endpoint the builder actually handed back, so a reordering fails loudly at bring-up instead of
/// silently bursting the control-frame endpoint (which would be wrong in the other direction too:
/// it would spend 4 KiB of staging on an endpoint that carries ~130-byte frames).
const BULK_OUT_EP_INDEX: usize = 2;

/// The `bRequest` value Windows uses for `GET_MS_OS_20_DESCRIPTOR`. Any non-zero byte; `0x01` is
/// the conventional pick and collides with nothing else we answer.
const MSOS_VENDOR_CODE: u8 = 0x01;

/// The device interface GUID Windows registers for this device, so an application can find it by
/// interface class rather than by VID/PID. Randomly generated once and **stable forever** — a
/// changed GUID is a different device to every Windows app that stored it.
const DEVICE_INTERFACE_GUID: &str = "{5A8B1CE4-2D3F-4E7A-9B10-6F2C8D41E9A3}";

// ============================ Resident buffers ============================
//
// All in `.bss` via the crate's `MaybeUninit` + `init_static` pattern, for the #677 reason: a
// buffer declared as a local in an async body gets a slot in the generated poll function's stack
// frame, allocated at entry on *every* poll, forever. The board has ~13 KB of measured stack margin
// — small buffers are exactly the thing that quietly eats it.

/// The driver's EP-OUT staging area: it copies each received packet here out of the core's shared
/// RX FIFO, and allocates one **armed transfer's worth** from it per OUT endpoint. Ours are EP0-OUT
/// (64) + control-OUT (512, one packet) + bulk-OUT ([`BULK_BURST_LEN`], because that endpoint arms
/// [`BULK_OUT_BURST_PACKETS`] at a time — see there). Undersizing this fails endpoint allocation, so
/// it is derived rather than guessed.
const EP_OUT_BUFFER_LEN: usize = 64 + MAX_PACKET as usize + BULK_BURST_LEN;
/// Buffer DMA is core-wide, so every IN endpoint owns one stable max-packet bounce slot until its
/// transfer completes: EP0-IN (64) + control-IN (512) + bulk-IN (512).
const EP_IN_BUFFER_LEN: usize = 64 + 2 * MAX_PACKET as usize;
const EP_BUFFER_LEN: usize = EP_OUT_BUFFER_LEN + EP_IN_BUFFER_LEN;

/// DWC2 buffer-DMA addresses must be DWORD-aligned. The driver sub-allocates both ends in aligned
/// units; this wrapper supplies the base guarantee that a plain `[u8; N]` static does not have.
#[repr(C, align(4))]
struct AlignedEndpointBuffer([u8; EP_BUFFER_LEN]);

static mut EP_BUFFER: MaybeUninit<AlignedEndpointBuffer> = MaybeUninit::uninit();

/// Configuration descriptor: config (9) + interface (9) + 4 × endpoint (7) = 46 bytes.
const CONFIG_DESC_LEN: usize = 96;
static mut CONFIG_DESC: MaybeUninit<[u8; CONFIG_DESC_LEN]> = MaybeUninit::uninit();
/// BOS descriptor: the header plus the 28-byte MS OS 2.0 platform capability descriptor.
const BOS_DESC_LEN: usize = 96;
static mut BOS_DESC: MaybeUninit<[u8; BOS_DESC_LEN]> = MaybeUninit::uninit();
/// MS OS 2.0 descriptor set: header + the `WINUSB` compatible id + the `DeviceInterfaceGUIDs`
/// registry property (the GUID string in UTF-16 is the bulk of it) ≈ 180 bytes.
const MSOS_DESC_LEN: usize = 256;
static mut MSOS_DESC: MaybeUninit<[u8; MSOS_DESC_LEN]> = MaybeUninit::uninit();
/// Control-transfer (EP0) data buffer.
///
/// Sized to §5.2.1's answer rather than to one EP0 packet: the stack hands this buffer to a
/// handler's `control_in` and then chunks whatever it wrote across EP0's 64-byte packets, so the
/// buffer is the response ceiling and the packet size is not. [`MAX_DEVICE_INFO`] is the widest
/// §5.2.1 permits (three 48-byte strings behind their length bytes); the rest is the standard
/// requests' data, which never exceeds one packet.
const CONTROL_BUF_LEN: usize = 256;
const _: () = assert!(CONTROL_BUF_LEN >= MAX_DEVICE_INFO, "EP0's buffer must hold §5.2.1's whole answer");
static mut CONTROL_BUF: MaybeUninit<[u8; CONTROL_BUF_LEN]> = MaybeUninit::uninit();

/// The EP0 vendor-request handler ([`device_info`]), pinned for the `'static` life the builder
/// borrows it for.
static mut INFO_HANDLER: MaybeUninit<DeviceInfoHandler> = MaybeUninit::uninit();

/// The `iSerialNumber` string, pinned for the `'static` life the descriptor borrows it for.
static mut SERIAL: MaybeUninit<heapless::String<16>> = MaybeUninit::uninit();

/// The USB plane's resident statics, summed for the budget assert in `main.rs` (the `usb` analogue
/// of the map and BLE terms). The driver's own endpoint bookkeeping (`StateStorage<16>`) lives
/// inside embassy-nrf and is not nameable here; it is a handful of wakers and atomics and shows up
/// in the linked `.bss` measurement, which is the authority.
///
/// The v4 adapter's own three buffers are [`v4::RESIDENT_BYTES`], summed in rather than left out:
/// they replace this module's old `CTRL_RX` + `BULK_BUF` pair and the 128 KiB arena arm the v1
/// upload staged through, and an itemization that split them across two files would make that
/// exchange impossible to read off either.
pub const RESIDENT_BYTES: usize = EP_BUFFER_LEN
    + CONFIG_DESC_LEN
    + BOS_DESC_LEN
    + MSOS_DESC_LEN
    + CONTROL_BUF_LEN
    + v4::RESIDENT_BYTES
    + core::mem::size_of::<DeviceInfoHandler>()
    + core::mem::size_of::<heapless::String<16>>()
    // [`VBUS_WAKER`] — 8 B, and itemized rather than waved through so the whole of #937's resident
    // cost is a named term instead of an unexplained step in the linked `.bss` gate.
    + core::mem::size_of::<AtomicWaker>();

// ============================ The VBUS gate (#936) ============================
//
// On glass, with J3 empty, the board faulted its way out of a boot — probe-rs reported
// `DAP FAULT (sticky_err, sticky_orun)` and lost the target, not a panic — and booted perfectly the
// moment a cable was plugged in. The mechanism is legible in the driver source:
//
//   - `UsbDevice::run` powers the Synopsys core up **only** on `Event::PowerDetected`, and
//     `Bus::poll` (embassy-nrf `usb/usbhs.rs`) does not emit that event until VBUS is detected.
//     With no cable the core stays behind `USBHS.ENABLE.CORE = 0` with its 24 MHz PHY clock
//     stopped, and its AHB slave does not answer.
//   - `Endpoint::wait_enabled` — the very first thing both `control::run` and `data_plane::run`
//     await — reads `USBHSCORE.DOEPCTL`. That read is the fault.
//
// With a cable the identical code is safe purely by poll order, which is why this was invisible
// until #930 put the plane in the default build: `join` polls `device.run()` first, and on this
// part `Bus::enable` contains no `await` that yields (the Synopsys `Bus::enable`/`disable` are
// documented no-op stubs), so the core is fully up before the endpoint futures are polled once.
//
// Hence two gates, and the second one matters as much as the first: an unplug takes the core back
// down (`PowerRemoved` → `Bus::disable` → `ENABLE.CORE/PHY = 0`, XO24M stopped) while the endpoint
// futures are still alive and will be re-polled the next time anything wakes this task.
//
//   1. **Before construction** — [`run`] parks until a cable is present, and only then builds.
//   2. **Before every delegated poll** — the guard in [`run`] re-reads VBUS synchronously in the
//      same instruction stream, so there is no window in which a poll of an endpoint future can
//      follow a power-down.
//
// Parking is **event-driven** (#937): the wait is on a VREGUSB interrupt ([`VbusEdge`]), so the
// cable-less case — the common one — costs no wake-ups at all. That is a power decision on a
// battery-powered device, and the reason it is even available is that #936 already made
// [`vbus_present`] the single level source: the driver reads VBUS through [`BoardVbusDetect`], so
// nothing of ours depends on embassy's private VBUS state and the vector is free to share.

/// Woken on **every** VREGUSB edge — plug or unplug — by [`VbusEdge`].
///
/// One task registers here ([`run`], through [`wait_for_vbus`]), so a single [`AtomicWaker`] is the
/// whole synchronisation story.
static VBUS_WAKER: AtomicWaker = AtomicWaker::new();

/// The USB service's VREGUSB edge handler: wake [`VBUS_WAKER`], and nothing else.
///
/// Bound **alongside** embassy's `vbus_detect::InterruptHandler` rather than instead of it (see
/// [`crate::board::UsbIrqs`]), which makes the ordering between the two irrelevant and is worth spelling out,
/// because "own the vector" is the obvious reading of #937 and it is the weaker design:
///
///   - **Ours reads nothing and clears nothing.** It cannot lose an edge to a handler that ran
///     first and already consumed `events_vbusdetected` / `events_vbusremoved`, because it never
///     looks at them. Every waiter downstream re-reads the *level* ([`vbus_present`]) anyway, so an
///     unconditional wake is not just sufficient, it is the only thing that can be correct here.
///   - **Embassy's still runs**, so `VREGUSB`'s events still get cleared (nobody else does it, and
///     an uncleared event is an interrupt storm), and embassy's private `BUS_WAKER` — the one
///     `Bus::poll` registers on — still gets woken. That last point is the load-bearing one: it
///     means the driver's wake path does not silently become a consequence of [`run`] happening to
///     `join` the device pump and the endpoint futures into *one* task. It is true today (see
///     [`wait_for_vbus`]) and it would stay true if someone split them tomorrow.
///
/// The cost of keeping embassy's handler is one atomic store and two wakes of wakers nothing of
/// ours registers on, on an event a human generates by hand.
pub(crate) struct VbusEdge;

impl interrupt::typelevel::Handler<interrupt::typelevel::VREGUSB> for VbusEdge {
    unsafe fn on_interrupt() {
        VBUS_WAKER.wake();
    }
}

/// Is a cable in J3 **right now**?
///
/// A *level* read of `VREGUSB.STATUS.VBUSDETECTED`, deliberately not the edge-driven flag
/// embassy's VREGUSB interrupt maintains: a gate that has to hold on every poll cannot depend on
/// an interrupt having already been serviced. embassy reads the same register the same way
/// (`vbus_detect.rs::initial_vbus_detected`) and for the same reason — `nrf-pac` 0.4 models
/// VREGUSB's tasks, events and interrupt registers but not `STATUS`, so the offset is spelled out.
///
/// Only meaningful once `VREGUSB.TASKS_START` has been issued, which [`HardwareVbusDetect::new`]
/// does; every caller here is downstream of that.
fn vbus_present() -> bool {
    /// `VREGUSB.STATUS` — the one register `nrf-pac` omits.
    const STATUS_OFFSET: usize = 0x400;
    /// `STATUS.VBUSDETECTED`.
    const VBUS_DETECTED: u32 = 1 << 2;
    // SAFETY: an aligned volatile read of a peripheral register embassy itself starts and reads,
    // inside a block the PAC hands out as a raw base pointer. No side effects.
    let status = unsafe { (pac::VREGUSB.as_ptr() as *const u32).add(STATUS_OFFSET / 4).read_volatile() };
    status & VBUS_DETECTED != 0
}

/// Park until a cable is present. Silent when one already is, and — the point of #937 — **free**
/// while it is not: no timer in the common path, just a task asleep on an interrupt.
///
/// Two properties make this loop correct, and both are about ordering:
///
///   - [`VBUS_WAKER`] is registered *before* the level is read. An edge landing in between is
///     therefore never lost: the wake either finds the registration (and re-polls us) or the read
///     that follows it already sees the new level.
///   - The outer `while` re-reads the level after every wake, so a wake for the *wrong* edge and a
///     wake that predates the first registration are the same thing — a reason to look again —
///     rather than two cases.
///
/// Those two together are why there is **no fallback timer**. A periodic re-check could only ever
/// help if a wake were lost, and the ordering above means one cannot be: the registration precedes
/// the read, and the read is of a *level*, not a latched edge. Confirmed on glass (Timo,
/// 2026-07-26) — plug, unplug and re-plug are all perceived immediately, so a timer here would be
/// wake-ups bought against a failure mode that does not exist. If USB ever *does* fail to notice a
/// cable, that is a broken interrupt path to fix, not a latency to paper over — see the board
/// README's failure-mode table.
async fn wait_for_vbus() {
    while !vbus_present() {
        poll_fn(|cx| {
            VBUS_WAKER.register(cx.waker());
            if vbus_present() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

/// The board's [`VbusDetect`], so the driver and the gate above cannot disagree.
///
/// Structurally, not probabilistically: [`HardwareVbusDetect`] answers from an `AtomicBool` its
/// interrupt handler maintains, which means the driver's view of the cable and [`vbus_present`]'s
/// are two variables that converge rather than one that is read twice. Handing the driver the same
/// level read removes the window in which the guard could wave an endpoint poll through *before*
/// `UsbDevice` had processed the matching `PowerDetected` and enabled the core.
///
/// [`HardwareVbusDetect::new`] is still constructed once in [`run`] — it is what issues
/// `VREGUSB.TASKS_START` and enables the vector whose handler wakes `Bus::poll` on a plug event.
/// This type replaces only the *answer*, never the arming.
pub(crate) struct BoardVbusDetect;

impl VbusDetect for BoardVbusDetect {
    fn is_usb_detected(&self) -> bool {
        vbus_present()
    }

    /// Matches embassy's own LM20 implementation exactly: on this part there is no separate
    /// "regulator output ready" signal to wait for, so the answer is the VBUS level, now.
    async fn wait_power_ready(&mut self) -> Result<(), ()> {
        if vbus_present() {
            Ok(())
        } else {
            Err(())
        }
    }
}

// ============================ Bring-up ============================

/// The concrete driver this board's USBHS produces, and its endpoint types. embassy-nrf keeps the
/// Synopsys endpoint type private, so they are named through the trait's associated types.
type UsbhsBusDriver = UsbhsDriver<'static, BoardVbusDetect>;
pub(crate) type EpIn = <UsbhsBusDriver as embassy_usb::driver::Driver<'static>>::EndpointIn;
pub(crate) type EpOut = <UsbhsBusDriver as embassy_usb::driver::Driver<'static>>::EndpointOut;

/// Everything [`run`] needs after enumeration is wired: the device to pump and the two pipes.
struct UsbPlane {
    device: UsbDevice<'static, UsbhsBusDriver>,
    ctrl_in: EpIn,
    ctrl_out: EpOut,
    bulk_in: EpIn,
    bulk_out: EpOut,
}

/// Build the driver, the descriptors and the four endpoints.
///
/// `#[inline(never)]` for the #677 reason the BLE stack documents at length: the [`Builder`] holds
/// the three descriptor writers plus the interface/handler vectors, and constructing it inline in
/// [`run`]'s async body would reserve its slot in that task's poll frame at entry, on every poll,
/// for the rest of the device's life. Here it lives in a transient frame that is popped before the
/// first `await`.
///
/// Called **only with VBUS present** — see the gate above. Nothing here reads the USBHS core
/// today (`UsbhsDriver::new`, endpoint allocation and `Builder::build` are all bookkeeping over
/// `.bss`), but that is an implementation detail of two upstream crates, not a contract, and the
/// cost of waiting first is one boot log line.
///
/// # Safety
/// Sole writer of every static above; called exactly once, from [`run`].
#[inline(never)]
fn build_plane(usb_p: Peri<'static, peripherals::USBHS>) -> UsbPlane {
    // SAFETY: each slot is written exactly once here, and the returned `&'static mut` is the sole
    // reference — `run` is spawned once from `main`.
    let ep_buffer =
        unsafe { init_static(core::ptr::addr_of_mut!(EP_BUFFER), AlignedEndpointBuffer([0u8; EP_BUFFER_LEN])) };
    let config_desc = unsafe { init_static(core::ptr::addr_of_mut!(CONFIG_DESC), [0u8; CONFIG_DESC_LEN]) };
    let bos_desc = unsafe { init_static(core::ptr::addr_of_mut!(BOS_DESC), [0u8; BOS_DESC_LEN]) };
    let msos_desc = unsafe { init_static(core::ptr::addr_of_mut!(MSOS_DESC), [0u8; MSOS_DESC_LEN]) };
    let control_buf = unsafe { init_static(core::ptr::addr_of_mut!(CONTROL_BUF), [0u8; CONTROL_BUF_LEN]) };
    let serial: &'static heapless::String<16> =
        unsafe { init_static(core::ptr::addr_of_mut!(SERIAL), identity::serial_string()) };

    // The driver forces `vbus_detection = false` itself on this part (VBUS events arrive through
    // VREGUSB, not the OTG core's session events), so the default config is otherwise the right one.
    // The one field we set is the fork's (#1173): arm the **bulk OUT** endpoint — and only that one
    // — as a burst of [`BULK_OUT_BURST_PACKETS`] packets, so it stops NAKing between them while the
    // CPU advances the transfer and services the card. Everything else on this device (EP0, the control
    // frame pipe, both IN endpoints) keeps stock one-packet arming.
    let mut usb_config = nrf_usb::Config::default();
    // Let the controller DMA directly between USB SRAM and the endpoint
    // buffers. This removes the per-packet CPU copy from the upload hot path.
    usb_config.buffer_dma = true;
    usb_config.out_burst_endpoints = 1 << BULK_OUT_EP_INDEX;
    usb_config.out_burst_packets = BULK_OUT_BURST_PACKETS;
    // [`BoardVbusDetect`] rather than embassy's `HardwareVbusDetect` so the driver and the poll
    // guard read the one register, not two views of it — the arming still happened in [`run`].
    let driver = UsbhsDriver::new(usb_p, crate::board::UsbIrqs, BoardVbusDetect, &mut ep_buffer.0, usb_config);

    let mut config = Config::new(VENDOR_ID, PRODUCT_ID);
    config.manufacturer = Some(MANUFACTURER);
    config.product = Some(PRODUCT);
    config.serial_number = Some(serial.as_str());
    // A single vendor-specific function, declared at the *device* level and explicitly not a
    // composite device. This is what makes the MS OS 2.0 descriptor set below a plain device-level
    // set (Microsoft's documented shape for a non-composite device) rather than a function subset,
    // and it keeps Windows from loading the generic composite parent driver in front of WinUSB.
    config.device_class = 0xFF;
    config.device_sub_class = 0x00;
    config.device_protocol = 0x00;
    // §5.2: the device descriptor's `bcdDevice` carries the USB-binding major in its high byte.
    // This is half of how a client settles the binding **before** a record is exchanged; the other
    // half is `bInterfaceProtocol` on the alt setting below. §3's frame major remains 4 and is
    // checked independently after record reassembly.
    config.device_release = u16::from(USB_BINDING_MAJOR) << 8;
    config.composite_with_iads = false;
    // Bus-powered from the host, like every other small USB peripheral; 100 mA is the pre-enumeration
    // budget every host grants unconditionally. (`bcd_usb` stays at the 2.1 default — declaring 2.1
    // is what tells the host to fetch the BOS descriptor the MS OS 2.0 set hangs off.)
    config.self_powered = false;
    config.max_power = 100;

    let mut builder = Builder::new(driver, config, config_desc, bos_desc, msos_desc, control_buf);

    // ---- MS OS 2.0 descriptors: Windows binds WinUSB with no .inf, no Zadig, no driver install.
    // Without these, a vendor-class interface on Windows shows up as an unknown device and the
    // browser cannot open it at all. `WINUSB` is the compatible id that selects the driver; the
    // `DeviceInterfaceGUIDs` registry property is what makes the interface enumerable by
    // applications afterwards. Both are device-level features because this is a single-function,
    // non-composite device.
    builder.msos_descriptor(windows_version::WIN8_1, MSOS_VENDOR_CODE);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&[DEVICE_INTERFACE_GUID]),
    ));

    // ---- The one interface and its four endpoints. **Allocation order is the wire contract**:
    // embassy hands out endpoint numbers in order, and the host pairs "lowest IN with lowest OUT"
    // as control and the next pair as bulk. Reordering these four lines silently swaps the planes.
    let (ctrl_in, ctrl_out, bulk_in, bulk_out, interface_number) = {
        let mut function = builder.function(0xFF, 0x00, 0x00);
        let mut interface = function.interface();
        let interface_number = interface.interface_number();
        // §5.2: the one place the USB-binding major is stated on the interface.
        let mut alt = interface.alt_setting(0xFF, 0x00, USB_BINDING_MAJOR, None);
        let ctrl_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let ctrl_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        let bulk_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let bulk_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        (ctrl_in, ctrl_out, bulk_in, bulk_out, interface_number)
    };

    // §5.2.1's EP0 vendor request. Device-level, because that is where embassy routes every request
    // the standard stack does not answer itself; the handler filters on the interface number above,
    // which is what makes it the *interface* request §5.2.1 specifies.
    //
    // SAFETY: sole writer of `INFO_HANDLER`; `build_plane` is called exactly once, from `run`.
    let info: &'static mut DeviceInfoHandler = unsafe {
        init_static(core::ptr::addr_of_mut!(INFO_HANDLER), DeviceInfoHandler { interface: interface_number })
    };
    builder.handler(info);

    // The burst mask above named an endpoint *index*, and indices are handed out in the allocation
    // order those four lines fix. Check the one the builder actually returned: a reordering that
    // slipped past the wire-contract comment would otherwise burst the wrong endpoint — the bulk
    // pipe back to one packet at a time (silently, at 1.4 MB/s) and 4 KiB of staging spent on
    // a channel whose widest message is a 100-byte `PUT`.
    assert_eq!(
        embassy_usb::driver::Endpoint::info(&bulk_out).addr.index(),
        BULK_OUT_EP_INDEX,
        "bulk OUT landed on the wrong endpoint index — the burst mask in `build_plane` is keyed on it"
    );

    UsbPlane { device: builder.build(), ctrl_in, ctrl_out, bulk_in, bulk_out }
}

/// Bring the USB device up and run it forever: the enumeration pump, the control-frame loop, and
/// the bulk object stream, all three joined on the thread-mode executor beside the ride loop and
/// the BLE stack.
///
/// **Cable-driven** (#936): nothing but VBUS detection is armed until a cable is in J3, and the
/// endpoint futures are polled only while one is — see the gate above. A boot with J3 empty must
/// reach the ride loop exactly as it did before this plane existed, and it says so in the log
/// rather than going quiet: a device that silently has no USB is indistinguishable from one whose
/// USB is broken, and that ambiguity is what let this ship.
///
/// An **embassy task**, not a plain future, and reached through a trampoline
/// ([`crate::spawn_usb_stack`]) — the same #677 discipline the BLE stack documents: a task's state
/// machine belongs in its own `.bss` pool, and the token construction belongs somewhere shallow.
#[embassy_executor::task]
pub async fn run(usb_p: Peri<'static, peripherals::USBHS>) -> ! {
    // Arm VBUS detection, and *only* that: `HardwareVbusDetect::new` touches VREGUSB (clear the two
    // events, unmask them, `TASKS_START`) and enables its vector. That is the entire hardware
    // footprint of a cable-less boot from here on — the value itself is discarded, because
    // `BoardVbusDetect` is what answers questions (see its doc).
    //
    // Arming is still embassy's, deliberately: it is also what enables the vector `VbusEdge` rides
    // on, so the event-driven park (#937) comes for free out of the same call rather than out of a
    // second, hand-rolled copy of these four register writes that could drift from it.
    //
    // Logged *before* the touch, not after, and this is deliberate. The bug this fixes was
    // diagnosed by which log line failed to appear, and that bisect cost a round trip because the
    // first USB hardware access had nothing in front of it. If a cable-less boot ever dies here
    // again, this line is the last one printed and VREGUSB — not USBHS — is the culprit, which is
    // the one reading the analysis below cannot rule out from source alone.
    info!("usb: arming VBUS detect (VREGUSB); no USBHS access until a cable is present");
    let _vbus_armed = HardwareVbusDetect::new(crate::board::UsbIrqs);

    let cable_present = vbus_present();
    set_usb_radio_inhibited(cable_present);
    if !cable_present {
        info!("usb: no VBUS on J3 — device plane parked; it comes up when a cable is plugged in");
        wait_for_vbus().await;
        set_usb_radio_inhibited(true);
    }
    info!("usb: VBUS present — BLE radio parked; bringing the device plane up");

    let UsbPlane { mut device, ctrl_in, ctrl_out, bulk_in, bulk_out } = build_plane(usb_p);
    info!(
        "usb: device plane up — {=u16:04x}:{=u16:04x}, serial '{}', HS bulk {} B",
        VENDOR_ID,
        PRODUCT_ID,
        identity::serial_string().as_str(),
        MAX_PACKET
    );

    // Built once and pinned, not rebuilt per cable: the adapter owns its four endpoints for the life
    // of the task and re-arms them across an unplug (`wait_enabled` at the top of its loop). The
    // cable cycle is expressed by *whether it is polled*, which keeps its `.bss` footprint a
    // property of the image rather than of how often someone plugs a cable in.
    let adapter = v4::serve_objects(ctrl_in, ctrl_out, bulk_in, bulk_out);
    let mut adapter = core::pin::pin!(adapter);

    join(device.run(), async {
        loop {
            // Serve while the cable is in. The VBUS re-read sits *before* the delegated poll, in
            // the same instruction stream, which is what closes the unplug race: the outer `join`
            // polls `device.run()` first, so the core may have been powered down by `PowerRemoved`
            // microseconds ago, inside this very pass.
            poll_fn(|cx| {
                if !vbus_present() {
                    return Poll::Ready(());
                }
                match adapter.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    // The adapter is `-> !`.
                    Poll::Ready(_) => unreachable!(),
                }
            })
            .await;
            // Reached when the cable goes: `VbusEdge` wakes this task on the removal edge and the
            // guard above re-reads the level. If a wake were somehow missed, the failure mode is a
            // plane that stays parked — never one that reads a powered-down core.
            warn!("usb: VBUS removed — device plane parked, endpoints idle until a cable returns");
            set_usb_radio_inhibited(false);
            // **The adapter is only parked here, not dropped**, so it cannot be what tells the
            // engine the link is gone: it is still suspended inside a record read and resumes when
            // a cable returns. What settles a half-landed transfer is the endpoint disable the host
            // controller raises on the same edge — the reader's next poll answers `Disabled`, the
            // driver ends, and `LinkLost` releases the allocation (§3.8's third form of cancel).
            // Nothing is resumed across the gap; §1 refuses resume outright.
            //
            // Asleep on the VREGUSB vector, not on a timer (#937). The device pump keeps its place
            // in this same `join` while we are parked, and that is fine in both directions: the
            // return edge wakes the *task*, which re-polls `device.run()`, whose `Bus::poll`
            // observes VBUS as a **level read through `BoardVbusDetect`** (embassy-nrf
            // `usb/usbhs.rs`, both arms of `poll`) rather than as a wait on any state private to
            // embassy. Its `BUS_WAKER` registration is a way to *be* woken, not the thing it looks
            // at — and it goes on being woken regardless, because embassy's handler is still bound
            // next to ours.
            wait_for_vbus().await;
            set_usb_radio_inhibited(true);
            info!("usb: VBUS back — BLE radio parked; device plane serving again");
        }
    })
    .await;
    // `UsbDevice::run` is `-> !`, so the join never completes.
    unreachable!()
}
