//! The USB device plane: the nRF54LM20's USBHS carrying flat-store frames, so a browser (WebUSB)
//! or the desktop app can push a map, a route or a firmware image to a plugged-in device. The
//! adapter owns record boundaries, pacing, timeouts and drain; the engine beside the card decides
//! what a frame means.
//!
//! One vendor-specific interface (class `0xFF`, `bInterfaceProtocol = 5`) with four bulk endpoints,
//! allocated in this order because the host reads the lowest IN/OUT pair as the control channel and
//! the next pair as the stream channel: 0x81/0x01 carries control records, 0x82/0x02 carries stream
//! records, both framed by [`records`]. The device information a host reads before it exchanges a
//! record is an EP0 vendor request instead; see [`device_info`].
//!
//! The USBHS is a high-speed core, so every bulk endpoint is 512 bytes by USB rule and there is no
//! full-speed fallback to size for. Packet boundaries carry no protocol meaning, which is what lets
//! an 8 KiB stream record exist on a 512-byte endpoint.
//!
//! The device must boot and ride with nothing plugged into J3. Nothing in this module may touch the
//! USBHS core unless a cable is present; see [`vbus_present`].
//!
//! D+/D-/VBUS/TXRTUNE are dedicated USBHS pins, so nothing in the board's pin plan moves. The
//! driver needs AHB >= 30 MHz and the board runs 128 MHz. It starts the XO24M oscillator itself
//! while enabled. The `USBHS` and `VREGUSB` vectors overlap neither MPSL nor the SDC.

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

/// Development USB vendor / product id: `0x1209` is [pid.codes](https://pid.codes) and `0x0001` is
/// its documented prototype pair. A real id moves two constants: [`PRODUCT_ID`] here, and
/// `OBC_USB_FILTERS` in `builder/app/src/lib/usb/webusb.ts`.
const VENDOR_ID: u16 = 0x1209;
const PRODUCT_ID: u16 = 0x0001;

/// The `iManufacturer` / `iProduct` strings. `iSerialNumber` is the FICR device id, which is what
/// makes two boards distinguishable in the browser's device chooser.
const MANUFACTURER: &str = "OpenBikeComputer";
const PRODUCT: &str = "OpenBikeComputer";

/// Every bulk endpoint on a high-speed device is 512 bytes: a USB rule, not a choice.
///
/// It is not a frame ceiling. A record spans packets, so what this bounds is one `write` call on an
/// IN endpoint (one call is one packet on this driver) and the arming of the control OUT endpoint.
const MAX_PACKET: u16 = 512;

/// How many max packets the bulk OUT endpoint arms at once: the upload throughput dial, and the
/// reason `embassy-usb-synopsys-otg` is vendored.
///
/// Stock, the driver arms one packet and re-arms it only after this firmware copied the packet out
/// and cleared NAK, so the endpoint NAKs for a full ISR-to-poll round trip every 512 B. Arming N
/// packets amortises that round trip over N. At 16, one burst carries a complete stream record, so
/// the endpoint is re-armed once per record and not halfway through one.
///
/// The RAM cost is two buffers of `N × 512 B`. The setting must also fit the core's RX FIFO of
/// 3040 words, of which a bursting endpoint takes `N × 129`, so 16 is the last rung that fits.
///
/// To sweep it: change this line, re-pin `compile_time_allocations.usb_named`,
/// `resident_ram_max`/`measured_resident` (they move by `2 × 512 × ΔN`) and `residual_stack_min` in
/// `firmware/tools/resource_baseline.json` on both profiles, then read the `~{} kB/s` line the v4
/// adapter prints over RTT. A host progress estimate is not a substitute.
const BULK_OUT_BURST_PACKETS: u16 = 16;

const BULK_BURST_LEN: usize = BULK_OUT_BURST_PACKETS as usize * MAX_PACKET as usize;

/// Payload bytes combined before the flat store sees one card write: eight stream records and one
/// 128-block CMD25, which is the efficient width of the sEMMC path. The bytes are an arm of the
/// existing scratch arena, not additional resident RAM.
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

/// The endpoint index the bulk OUT pipe lands on, which is what the driver's burst mask is keyed
/// on ([`Config::out_burst_endpoints`](nrf_usb::Config)). It is 2 because of the allocation order
/// in [`build_plane`], which is also the host's wire contract. [`build_plane`] asserts it against
/// the endpoint the builder handed back, so a reordering fails at bring-up instead of bursting the
/// control-frame endpoint.
const BULK_OUT_EP_INDEX: usize = 2;

/// The `bRequest` value Windows uses for `GET_MS_OS_20_DESCRIPTOR`. Any non-zero byte.
const MSOS_VENDOR_CODE: u8 = 0x01;

/// The device interface GUID Windows registers, so an application can find this device by
/// interface class and not by VID/PID. It must stay stable: a changed GUID is a different device to
/// every Windows application that stored it.
const DEVICE_INTERFACE_GUID: &str = "{5A8B1CE4-2D3F-4E7A-9B10-6F2C8D41E9A3}";

// All in `.bss` through the crate's `MaybeUninit` + `init_static` pattern: a buffer declared as a
// local in an async body gets a slot in the generated poll function's stack frame, allocated at
// entry on every poll, forever. The board has about 13 KB of measured stack margin.

/// The driver's EP-OUT staging area: it copies each received packet here out of the core's shared
/// RX FIFO, and allocates one armed transfer's worth from it per OUT endpoint. Undersizing this
/// fails endpoint allocation, so it is derived and not guessed.
const EP_OUT_BUFFER_LEN: usize = 64 + MAX_PACKET as usize + BULK_BURST_LEN;
/// Buffer DMA is core-wide, so every IN endpoint owns one stable max-packet bounce slot until its
/// transfer completes.
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
/// MS OS 2.0 descriptor set: header, the `WINUSB` compatible id, and the `DeviceInterfaceGUIDs`
/// registry property.
const MSOS_DESC_LEN: usize = 256;
static mut MSOS_DESC: MaybeUninit<[u8; MSOS_DESC_LEN]> = MaybeUninit::uninit();
/// Control-transfer (EP0) data buffer. The stack hands this buffer to a handler's `control_in` and
/// then chunks what it wrote across EP0's 64-byte packets, so this buffer is the response ceiling
/// and the packet size is not. [`MAX_DEVICE_INFO`] is the widest answer; the standard requests'
/// data never exceeds one packet.
const CONTROL_BUF_LEN: usize = 256;
const _: () = assert!(CONTROL_BUF_LEN >= MAX_DEVICE_INFO, "EP0's buffer must hold §5.2.1's whole answer");
static mut CONTROL_BUF: MaybeUninit<[u8; CONTROL_BUF_LEN]> = MaybeUninit::uninit();

/// The EP0 vendor-request handler ([`device_info`]), pinned for the `'static` life the builder
/// borrows it for.
static mut INFO_HANDLER: MaybeUninit<DeviceInfoHandler> = MaybeUninit::uninit();

#[cfg(feature = "peak-view-demo")]
struct EnumerationDiagnostics;

#[cfg(feature = "peak-view-demo")]
impl embassy_usb::Handler for EnumerationDiagnostics {
    fn reset(&mut self) {
        info!("usb-enum: bus reset");
    }

    fn addressed(&mut self, address: u8) {
        info!("usb-enum: addressed {=u8}", address);
    }

    fn configured(&mut self, configured: bool) {
        info!("usb-enum: configured {=bool}", configured);
    }
}

#[cfg(feature = "peak-view-demo")]
static mut ENUMERATION_DIAGNOSTICS: MaybeUninit<EnumerationDiagnostics> = MaybeUninit::uninit();

/// The `iSerialNumber` string, pinned for the `'static` life the descriptor borrows it for.
static mut SERIAL: MaybeUninit<heapless::String<16>> = MaybeUninit::uninit();

/// The USB plane's resident statics, summed into the resource report. The driver's own endpoint
/// bookkeeping (`StateStorage<16>`) lives inside embassy-nrf and is not nameable here; it shows up
/// in the linked `.bss` measurement, which is the authority.
pub const RESIDENT_BYTES: usize = EP_BUFFER_LEN
    + CONFIG_DESC_LEN
    + BOS_DESC_LEN
    + MSOS_DESC_LEN
    + CONTROL_BUF_LEN
    + v4::RESIDENT_BYTES
    + core::mem::size_of::<DeviceInfoHandler>()
    + core::mem::size_of::<heapless::String<16>>()
    // [`VBUS_WAKER`].
    + core::mem::size_of::<AtomicWaker>();

// The VBUS gate. With J3 empty, the first endpoint read faults the bus and loses the target:
// `UsbDevice::run` powers the Synopsys core up only on `Event::PowerDetected`, and with no cable
// the core stays behind `USBHS.ENABLE.CORE = 0` with its 24 MHz PHY clock stopped, so its AHB
// slave does not answer. `Endpoint::wait_enabled`, the first thing the endpoint futures await,
// reads `USBHSCORE.DOEPCTL`.
//
// Hence two gates, and the second matters as much as the first, because an unplug takes the core
// back down while the endpoint futures are still alive:
//
//   1. [`run`] parks until a cable is present, and only then builds.
//   2. The guard in [`run`] re-reads VBUS synchronously in the same instruction stream, so no poll
//      of an endpoint future can follow a power-down.
//
// Parking is event-driven: the wait is on a VREGUSB interrupt ([`VbusEdge`]), so the common
// cable-less case costs no wake-ups. This is possible because [`vbus_present`] is the single level
// source: the driver reads VBUS through [`BoardVbusDetect`], so nothing of ours depends on
// embassy's private VBUS state and the vector is free to share.

/// Woken on every VREGUSB edge, plug or unplug, by [`VbusEdge`]. One task registers here.
static VBUS_WAKER: AtomicWaker = AtomicWaker::new();

/// The USB service's VREGUSB edge handler: wake [`VBUS_WAKER`], and nothing else.
///
/// It is bound alongside embassy's `vbus_detect::InterruptHandler`, not instead of it (see
/// [`crate::board::UsbIrqs`]), which makes the order between the two irrelevant. Ours reads and
/// clears nothing, so it cannot lose an edge to a handler that ran first; every waiter re-reads the
/// level. Embassy's handler still clears VREGUSB's events, and an uncleared event is an interrupt
/// storm.
pub(crate) struct VbusEdge;

impl interrupt::typelevel::Handler<interrupt::typelevel::VREGUSB> for VbusEdge {
    unsafe fn on_interrupt() {
        VBUS_WAKER.wake();
    }
}

/// Is a cable in J3 right now?
///
/// A level read of `VREGUSB.STATUS.VBUSDETECTED`, and not the edge-driven flag embassy's VREGUSB
/// interrupt keeps: a gate that must hold on every poll cannot depend on an interrupt having been
/// serviced. `nrf-pac` does not model `STATUS`, so the offset is spelled out. The answer is
/// meaningful only after `VREGUSB.TASKS_START`, which [`HardwareVbusDetect::new`] issues.
fn vbus_present() -> bool {
    const STATUS_OFFSET: usize = 0x400;
    const VBUS_DETECTED: u32 = 1 << 2;
    // SAFETY: an aligned volatile read of a peripheral register that embassy itself starts and
    // reads, from the raw base pointer the PAC hands out. No side effects.
    let status = unsafe { (pac::VREGUSB.as_ptr() as *const u32).add(STATUS_OFFSET / 4).read_volatile() };
    status & VBUS_DETECTED != 0
}

/// Park until a cable is present. There is no timer in the common path, only a task asleep on an
/// interrupt.
///
/// [`VBUS_WAKER`] is registered before the level is read, so an edge that lands between the two is
/// never lost: the wake either finds the registration, or the read that follows already sees the
/// new level. The `while` re-reads the level after every wake, so a wake for the wrong edge is only
/// a reason to look again. Together, these two are why there is no fallback timer.
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
/// [`HardwareVbusDetect`] answers from an `AtomicBool` its interrupt handler keeps, which makes the
/// driver's view of the cable and [`vbus_present`]'s two variables that converge, and not one that
/// is read twice. This type replaces only the answer: [`HardwareVbusDetect::new`] in [`run`] still
/// issues `VREGUSB.TASKS_START` and enables the vector.
pub(crate) struct BoardVbusDetect;

impl VbusDetect for BoardVbusDetect {
    fn is_usb_detected(&self) -> bool {
        vbus_present()
    }

    /// On this part there is no separate "regulator output ready" signal to wait for, so the
    /// answer is the VBUS level now.
    async fn wait_power_ready(&mut self) -> Result<(), ()> {
        if vbus_present() {
            Ok(())
        } else {
            Err(())
        }
    }
}

/// The concrete driver this board's USBHS produces, and its endpoint types. embassy-nrf keeps the
/// Synopsys endpoint type private, so they are named through the trait's associated types.
type UsbhsBusDriver = UsbhsDriver<'static, BoardVbusDetect>;
pub(crate) type EpIn = <UsbhsBusDriver as embassy_usb::driver::Driver<'static>>::EndpointIn;
pub(crate) type EpOut = <UsbhsBusDriver as embassy_usb::driver::Driver<'static>>::EndpointOut;

struct UsbPlane {
    device: UsbDevice<'static, UsbhsBusDriver>,
    ctrl_in: EpIn,
    ctrl_out: EpOut,
    bulk_in: EpIn,
    bulk_out: EpOut,
}

/// Build the driver, the descriptors and the four endpoints.
///
/// `#[inline(never)]` because the [`Builder`] holds the three descriptor writers and the
/// interface and handler vectors: constructed inline in [`run`]'s async body, it would reserve its
/// slot in that task's poll frame at entry, on every poll, for the life of the device.
///
/// Called only with VBUS present; see the gate above.
///
/// # Safety
/// Sole writer of every static above; called exactly once, from [`run`].
#[inline(never)]
fn build_plane(usb_p: Peri<'static, peripherals::USBHS>) -> UsbPlane {
    // SAFETY: each slot is written exactly once here, and the returned `&'static mut` is the sole
    // reference, because `run` is spawned once from `main`.
    let ep_buffer =
        unsafe { init_static(core::ptr::addr_of_mut!(EP_BUFFER), AlignedEndpointBuffer([0u8; EP_BUFFER_LEN])) };
    let config_desc = unsafe { init_static(core::ptr::addr_of_mut!(CONFIG_DESC), [0u8; CONFIG_DESC_LEN]) };
    let bos_desc = unsafe { init_static(core::ptr::addr_of_mut!(BOS_DESC), [0u8; BOS_DESC_LEN]) };
    let msos_desc = unsafe { init_static(core::ptr::addr_of_mut!(MSOS_DESC), [0u8; MSOS_DESC_LEN]) };
    let control_buf = unsafe { init_static(core::ptr::addr_of_mut!(CONTROL_BUF), [0u8; CONTROL_BUF_LEN]) };
    let serial: &'static heapless::String<16> =
        unsafe { init_static(core::ptr::addr_of_mut!(SERIAL), identity::serial_string()) };

    // The driver forces `vbus_detection = false` itself on this part, because VBUS events arrive
    // through VREGUSB and not through the OTG core's session events. The one field we set arms the
    // bulk OUT endpoint, and only that one, as a burst of [`BULK_OUT_BURST_PACKETS`] packets, so it
    // stops NAKing between them while the CPU advances the transfer and services the card.
    let mut usb_config = nrf_usb::Config::default();
    // DMA straight between USB SRAM and the endpoint buffers: no per-packet CPU copy.
    usb_config.buffer_dma = true;
    usb_config.out_burst_endpoints = 1 << BULK_OUT_EP_INDEX;
    usb_config.out_burst_packets = BULK_OUT_BURST_PACKETS;
    // [`BoardVbusDetect`], so the driver and the poll guard read one register and not two views of
    // it. The arming still happened in [`run`].
    let driver = UsbhsDriver::new(usb_p, crate::board::UsbIrqs, BoardVbusDetect, &mut ep_buffer.0, usb_config);

    let mut config = Config::new(VENDOR_ID, PRODUCT_ID);
    config.manufacturer = Some(MANUFACTURER);
    config.product = Some(PRODUCT);
    config.serial_number = Some(serial.as_str());
    // A single vendor-specific function at the device level, and explicitly not a composite
    // device. This makes the MS OS 2.0 descriptor set below a device-level set, and it keeps
    // Windows from loading the generic composite parent driver in front of WinUSB.
    config.device_class = 0xFF;
    config.device_sub_class = 0x00;
    config.device_protocol = 0x00;
    // The device descriptor's `bcdDevice` carries the USB-binding major in its high byte. A client
    // settles the binding from this and from `bInterfaceProtocol` below, before a record moves. The
    // frame major is separate and is checked after record reassembly.
    config.device_release = u16::from(USB_BINDING_MAJOR) << 8;
    config.composite_with_iads = false;
    // Bus-powered from the host; 100 mA is the pre-enumeration budget every host grants. `bcd_usb`
    // stays at the 2.1 default, which is what tells the host to fetch the BOS descriptor the MS OS
    // 2.0 set hangs off.
    config.self_powered = false;
    config.max_power = 100;

    let mut builder = Builder::new(driver, config, config_desc, bos_desc, msos_desc, control_buf);

    // MS OS 2.0 descriptors: Windows binds WinUSB with no .inf and no driver install. Without
    // them a vendor-class interface is an unknown device and the browser cannot open it. `WINUSB`
    // is the compatible id that selects the driver; the `DeviceInterfaceGUIDs` registry property
    // makes the interface enumerable by applications.
    builder.msos_descriptor(windows_version::WIN8_1, MSOS_VENDOR_CODE);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&[DEVICE_INTERFACE_GUID]),
    ));

    // Allocation order is the wire contract: embassy hands out endpoint numbers in order, and the
    // host pairs the lowest IN with the lowest OUT as control and the next pair as bulk. To reorder
    // these four lines swaps the two planes.
    let (ctrl_in, ctrl_out, bulk_in, bulk_out, interface_number) = {
        let mut function = builder.function(0xFF, 0x00, 0x00);
        let mut interface = function.interface();
        let interface_number = interface.interface_number();
        // The one place the USB-binding major is stated on the interface.
        let mut alt = interface.alt_setting(0xFF, 0x00, USB_BINDING_MAJOR, None);
        let ctrl_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let ctrl_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        let bulk_in = alt.endpoint_bulk_in(None, MAX_PACKET);
        let bulk_out = alt.endpoint_bulk_out(None, MAX_PACKET);
        (ctrl_in, ctrl_out, bulk_in, bulk_out, interface_number)
    };

    // SAFETY: sole writer of `INFO_HANDLER`; `build_plane` is called exactly once, from `run`.
    let info: &'static mut DeviceInfoHandler = unsafe {
        init_static(core::ptr::addr_of_mut!(INFO_HANDLER), DeviceInfoHandler { interface: interface_number })
    };
    builder.handler(info);
    #[cfg(feature = "peak-view-demo")]
    {
        // SAFETY: built once alongside INFO_HANDLER; the zero-sized handler owns no buffers.
        let diagnostics =
            unsafe { init_static(core::ptr::addr_of_mut!(ENUMERATION_DIAGNOSTICS), EnumerationDiagnostics) };
        builder.handler(diagnostics);
    }

    // The burst mask above names an endpoint index, and indices follow the allocation order those
    // four lines fix. Check the one the builder returned: a reordering would burst the wrong
    // endpoint, and put the bulk pipe back to one packet at a time.
    assert_eq!(
        embassy_usb::driver::Endpoint::info(&bulk_out).addr.index(),
        BULK_OUT_EP_INDEX,
        "bulk OUT landed on the wrong endpoint index — the burst mask in `build_plane` is keyed on it"
    );

    UsbPlane { device: builder.build(), ctrl_in, ctrl_out, bulk_in, bulk_out }
}

/// Bring the USB device up and run it forever: the enumeration pump, the control-frame loop and
/// the bulk object stream, joined on the thread-mode executor beside the ride loop and the BLE
/// stack.
///
/// Cable-driven: nothing but VBUS detection is armed until a cable is in J3, and the endpoint
/// futures are polled only while one is. A boot with J3 empty must reach the ride loop, and it says
/// so in the log, because a device that silently has no USB looks like one whose USB is broken.
///
/// An embassy task, not a plain future, and reached through a trampoline
/// ([`crate::spawn_usb_stack`]): a task's state machine belongs in its own `.bss` pool, and the
/// token construction belongs somewhere shallow.
#[embassy_executor::task]
pub async fn run(usb_p: Peri<'static, peripherals::USBHS>) -> ! {
    // Arm VBUS detection, and only that: `HardwareVbusDetect::new` touches VREGUSB (clear the two
    // events, unmask them, `TASKS_START`) and enables its vector, which is also the vector
    // `VbusEdge` rides on. The value itself is discarded, because `BoardVbusDetect` answers.
    //
    // The log line comes before that touch: if a cable-less boot ever dies here, this line is the
    // last one printed and VREGUSB, not USBHS, is the cause.
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

    // Built once and pinned, not rebuilt per cable: the adapter owns its four endpoints for the
    // life of the task and re-arms them across an unplug. The cable cycle is expressed by whether
    // the adapter is polled.
    let adapter = v4::serve_objects(ctrl_in, ctrl_out, bulk_in, bulk_out);
    let mut adapter = core::pin::pin!(adapter);

    join(device.run(), async {
        loop {
            // Serve while the cable is in. The VBUS re-read sits before the delegated poll, in the
            // same instruction stream, which closes the unplug race: the outer `join` polls
            // `device.run()` first, so the core may have been powered down inside this same pass.
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
            // Reached when the cable goes. If a wake were missed, the failure mode is a plane that
            // stays parked, never one that reads a powered-down core.
            warn!("usb: VBUS removed — device plane parked, endpoints idle until a cable returns");
            set_usb_radio_inhibited(false);
            // The adapter is parked here, not dropped, so it is not what tells the engine the link
            // is gone: it stays suspended inside a record read and resumes when a cable returns.
            // What settles a half-landed transfer is the endpoint disable the host controller
            // raises on the same edge: the reader's next poll answers `Disabled`, the driver ends,
            // and `LinkLost` releases the allocation. Nothing is resumed across the gap.
            //
            // Asleep on the VREGUSB vector, not on a timer. The device pump keeps its place in this
            // same `join`: the return edge wakes the task, which re-polls `device.run()`, whose
            // `Bus::poll` reads the VBUS level through `BoardVbusDetect` and waits on no state
            // private to embassy.
            wait_for_vbus().await;
            set_usb_radio_inhibited(true);
            info!("usb: VBUS back — BLE radio parked; device plane serving again");
        }
    })
    .await;
    // `UsbDevice::run` is `-> !`, so the join never completes.
    unreachable!()
}
