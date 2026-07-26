//! The **USB device plane** (issue #889): the nRF54LM20's USBHS carrying the *same* companion
//! protocol the radio carries, so a browser (WebUSB) or the desktop app can push a map, a route or
//! a firmware image straight to a plugged-in device.
//!
//! ## Why this is a transport, not a protocol
//!
//! `obc-ble-interface-spec.md` principle #2 — the bulk channel is a raw byte pipe with **no
//! per-chunk framing** — is the whole reason this module is small. BLE's L2CAP CoC and a USB bulk
//! endpoint are the same thing (reliable, ordered, unframed), so the object stream needs *zero*
//! translation: the identical [`obc_ble::Receiver`] / [`obc_ble::StreamSender`] state machine, the
//! identical descriptors, the identical whole-object CRC-32, and the identical
//! `protocol-vectors/` fixtures. Everything that decides *what* a message means lives in
//! [`crate::link`], shared with the radio.
//!
//! Only one thing genuinely differs. BLE's control plane is GATT: seven separately-addressed
//! characteristics, where "which characteristic" is carried by the transport rather than by any
//! byte of ours. USB has one endpoint pair, so that routing becomes a byte — see [`control`] for
//! the envelope (ratified from C3/#902, which built the host client against it).
//!
//! ## Endpoint layout (the host contract)
//!
//! One vendor-specific interface (class `0xFF`), four bulk endpoints, allocated in this order so
//! the host's "lowest IN/OUT pair is control, the next is bulk" rule
//! (`packer/web_builder/frontend/src/lib/usb/webusb.ts::discoverLayout`) reads them correctly:
//!
//! | Endpoint | Direction | Carries |
//! | :-- | :-- | :-- |
//! | 0x81 / 0x01 | IN / OUT | control frames — one frame per transfer, `selector u8 · payload` |
//! | 0x82 / 0x02 | IN / OUT | the unframed object stream — BLE's CoC, byte for byte |
//!
//! The USBHS is a **high-speed** core (`PhyType::InternalHighSpeed`), so every bulk endpoint is
//! 512 bytes by USB rule; there is no full-speed fallback to size for.
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
//! **Zero GPIO cost**: D+/D−/VBUS/TXRTUNE are dedicated USBHS pins, so nothing in the board's pin
//! plan moves. The driver needs AHB ≥ 30 MHz (asserted in its `calculate_trdt`); the board runs
//! 128 MHz. It starts the **XO24M** oscillator itself while enabled, and claims two vectors —
//! `USBHS` and `VREGUSB` — neither of which MPSL or the SDC touch (MPSL takes `RADIO_0`,
//! `TIMER10`, `GRTC_3`, `CLOCK_POWER`, `SWI00`; the board's high-priority input executor is on
//! `SWI01`).
//!
//! ## Concurrency with the ride loop — the reason MSC was rejected
//!
//! The app keeps running while a transfer lands. Every store call locks the shared SD + settings
//! mutex only for its own duration and releases before the next endpoint `await`, so the ride
//! loop's map render interleaves between chunks exactly as it does for BLE. Mass Storage would
//! instead have handed the host raw block access and forced the firmware to release the card
//! entirely; firmware-mediated writes compose with the existing arbitration instead.

pub(crate) mod control;
pub(crate) mod data_plane;

use core::future::{poll_fn, Future};
use core::mem::MaybeUninit;
use core::task::Poll;

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_nrf::usb::vbus_detect::{HardwareVbusDetect, VbusDetect};
use embassy_nrf::usb::{self as nrf_usb, Driver as UsbhsDriver};
use embassy_nrf::{bind_interrupts, interrupt, pac, peripherals, Peri};
use embassy_sync::waitqueue::AtomicWaker;
use embassy_usb::msos::{self, windows_version};
use embassy_usb::{Builder, Config, UsbDevice};

use crate::init_static;
use crate::link::identity;

use control::ControlTx;

bind_interrupts!(struct Irqs {
    USBHS => nrf_usb::InterruptHandler<peripherals::USBHS>;
    // Two handlers on the one vector, and the order between them does not matter — see
    // [`VbusEdge`] for why ours reads nothing and clears nothing.
    VREGUSB => VbusEdge, nrf_usb::vbus_detect::InterruptHandler;
});

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
/// `packer/web_builder/frontend/src/lib/usb/webusb.ts`.
const VENDOR_ID: u16 = 0x1209;
const PRODUCT_ID: u16 = 0x0001;

/// The `iManufacturer` / `iProduct` strings. `iSerialNumber` is the FICR device id (see
/// [`identity::serial_string`]) — it is what makes two boards distinguishable in the browser's
/// device chooser and what lets a host remember *which* device it was granted.
const MANUFACTURER: &str = "OpenBikeComputer";
const PRODUCT: &str = "OpenBikeComputer";

/// Every bulk endpoint on a high-speed device is 512 bytes — the USB 2.0 rule, not a choice. It is
/// also the control plane's frame ceiling: a control frame is exactly one transfer, and the host
/// rejects a frame that would fill a whole packet (it could not tell the frame had ended without a
/// zero-length packet). The largest frame the protocol sends is a `config` write at ~130 bytes.
const MAX_PACKET: u16 = 512;

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
/// RX FIFO, and allocates `max_packet_size` from it **per OUT endpoint**. Ours are EP0-OUT (64) +
/// control-OUT (512) + bulk-OUT (512). Undersizing this panics at endpoint allocation, so it is
/// derived rather than guessed.
const EP_OUT_BUFFER_LEN: usize = 64 + 2 * MAX_PACKET as usize;
static mut EP_OUT_BUFFER: MaybeUninit<[u8; EP_OUT_BUFFER_LEN]> = MaybeUninit::uninit();

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
/// Control-transfer (EP0) data buffer. We register no custom control handler, so this only has to
/// hold the standard requests' data — one EP0 packet.
const CONTROL_BUF_LEN: usize = 64;
static mut CONTROL_BUF: MaybeUninit<[u8; CONTROL_BUF_LEN]> = MaybeUninit::uninit();

/// One received control frame. Sized to the endpoint because `EndpointOut::read` refuses a buffer
/// smaller than one max packet.
static mut CTRL_RX: MaybeUninit<[u8; MAX_PACKET as usize]> = MaybeUninit::uninit();
/// One bulk chunk, in either direction — the USB analogue of the CoC's SDU scratch.
static mut BULK_BUF: MaybeUninit<[u8; MAX_PACKET as usize]> = MaybeUninit::uninit();

/// The `iSerialNumber` string, pinned for the `'static` life the descriptor borrows it for.
static mut SERIAL: MaybeUninit<heapless::String<16>> = MaybeUninit::uninit();

/// The USB plane's resident statics, summed for the budget assert in `main.rs` (the `usb` analogue
/// of the map and BLE terms). The driver's own endpoint bookkeeping (`StateStorage<16>`) lives
/// inside embassy-nrf and is not nameable here; it is a handful of wakers and atomics and shows up
/// in the linked `.bss` measurement, which is the authority.
pub const RESIDENT_BYTES: usize = EP_OUT_BUFFER_LEN
    + CONFIG_DESC_LEN
    + BOS_DESC_LEN
    + MSOS_DESC_LEN
    + CONTROL_BUF_LEN
    + 2 * MAX_PACKET as usize
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

/// The board's VREGUSB interrupt handler: wake [`VBUS_WAKER`], and nothing else.
///
/// Bound **alongside** embassy's `vbus_detect::InterruptHandler` rather than instead of it (see
/// [`Irqs`]), which is what makes the ordering between the two irrelevant and is worth spelling out,
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
struct VbusEdge;

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
    let ep_out_buffer = unsafe { init_static(core::ptr::addr_of_mut!(EP_OUT_BUFFER), [0u8; EP_OUT_BUFFER_LEN]) };
    let config_desc = unsafe { init_static(core::ptr::addr_of_mut!(CONFIG_DESC), [0u8; CONFIG_DESC_LEN]) };
    let bos_desc = unsafe { init_static(core::ptr::addr_of_mut!(BOS_DESC), [0u8; BOS_DESC_LEN]) };
    let msos_desc = unsafe { init_static(core::ptr::addr_of_mut!(MSOS_DESC), [0u8; MSOS_DESC_LEN]) };
    let control_buf = unsafe { init_static(core::ptr::addr_of_mut!(CONTROL_BUF), [0u8; CONTROL_BUF_LEN]) };
    let serial: &'static heapless::String<16> =
        unsafe { init_static(core::ptr::addr_of_mut!(SERIAL), identity::serial_string()) };

    // The driver forces `vbus_detection = false` itself on this part (VBUS events arrive through
    // VREGUSB, not the OTG core's session events), so the default config is the right one.
    // [`BoardVbusDetect`] rather than embassy's `HardwareVbusDetect` so the driver and the poll
    // guard read the one register, not two views of it — the arming still happened in [`run`].
    let driver = UsbhsDriver::new(usb_p, Irqs, BoardVbusDetect, ep_out_buffer, nrf_usb::Config::default());

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
    let (ctrl_in, ctrl_out, bulk_in, bulk_out) = {
        let mut function = builder.function(0xFF, 0x00, 0x00);
        let mut interface = function.interface();
        let mut alt = interface.alt_setting(0xFF, 0x00, 0x00, None);
        let ctrl_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let ctrl_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        let bulk_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let bulk_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        (ctrl_in, ctrl_out, bulk_in, bulk_out)
    };

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
pub async fn run(usb_p: Peri<'static, peripherals::USBHS>, stores: crate::link::LinkStores) -> ! {
    let crate::link::LinkStores { shared, objects: store, epoch: store_epoch } = stores;

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
    let _vbus_armed = HardwareVbusDetect::new(Irqs);

    if !vbus_present() {
        info!("usb: no VBUS on J3 — device plane parked; it comes up when a cable is plugged in");
        wait_for_vbus().await;
    }
    info!("usb: VBUS present — bringing the device plane up");

    let UsbPlane { mut device, ctrl_in, ctrl_out, bulk_in, bulk_out } = build_plane(usb_p);
    info!(
        "usb: device plane up — {=u16:04x}:{=u16:04x}, serial '{}', HS bulk {} B",
        VENDOR_ID,
        PRODUCT_ID,
        identity::serial_string().as_str(),
        MAX_PACKET
    );

    // SAFETY: sole writer of each buffer; `run` is spawned once.
    let ctrl_rx = unsafe { init_static(core::ptr::addr_of_mut!(CTRL_RX), [0u8; MAX_PACKET as usize]) };
    let bulk_buf = unsafe { init_static(core::ptr::addr_of_mut!(BULK_BUF), [0u8; MAX_PACKET as usize]) };

    // Both planes send on the control IN endpoint — the control loop its replies, the data plane
    // its announces and terminal results — so it lives behind one async mutex. That serialisation
    // is also the ordering guarantee the host relies on: like BLE's single `status` CCCD, every
    // device → host control message shares one ordering domain.
    let tx = ControlTx::new(ctrl_in);

    // Built once and pinned, not rebuilt per cable: both halves own their endpoints for the life of
    // the task and already re-arm themselves across an unplug (`wait_enabled` at the top of each
    // loop, `link_reset` on the way round). The cable cycle is expressed by *whether they are
    // polled*, which keeps their signatures — and their `.bss` footprint — unchanged.
    let planes = join(
        control::run(&tx, ctrl_out, ctrl_rx, store, shared, store_epoch),
        data_plane::run(&tx, bulk_in, bulk_out, bulk_buf, store, shared),
    );
    let mut planes = core::pin::pin!(planes);

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
                match planes.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    // Both halves are `-> !`.
                    Poll::Ready(_) => unreachable!(),
                }
            })
            .await;
            // Reached when the cable goes: `VbusEdge` wakes this task on the removal edge and the
            // guard above re-reads the level. If a wake were somehow missed, the failure mode is a
            // plane that stays parked — never one that reads a powered-down core.
            warn!("usb: VBUS removed — device plane parked, endpoints idle until a cable returns");
            // Asleep on the VREGUSB vector, not on a timer (#937). The device pump keeps its place
            // in this same `join` while we are parked, and that is fine in both directions: the
            // return edge wakes the *task*, which re-polls `device.run()`, whose `Bus::poll`
            // observes VBUS as a **level read through `BoardVbusDetect`** (embassy-nrf
            // `usb/usbhs.rs`, both arms of `poll`) rather than as a wait on any state private to
            // embassy. Its `BUS_WAKER` registration is a way to *be* woken, not the thing it looks
            // at — and it goes on being woken regardless, because embassy's handler is still bound
            // next to ours.
            wait_for_vbus().await;
            info!("usb: VBUS back — device plane serving again");
        }
    })
    .await;
    // `UsbDevice::run` is `-> !`, so the join never completes.
    unreachable!()
}
