//! A device that owns its card: the shape the browser and the dev harness need.
//!
//! [`Device`](crate::Device) is the assembly over whatever card the caller holds — the Rust harness
//! holds a `SparseDisk` and the test outlives it. Nothing outlives a device behind a wasm boundary,
//! so this one holds the card itself, which is what makes a reboot a reboot (drop the engine and the
//! store, lose what was never synced, mount the same durable bytes again) rather than a client that
//! reconnected.

use obc_link::flat::{Ceilings, Link};
use obc_storage::flat::{EntryFlags, EntryMeta, StoreId};

use crate::card::Card;
use crate::{blocks_for, AllowArm, Device, OpenPolicy, Reaction, TracedRequest, EXTENTS, STORE, USB_RECORD_CEILING};

/// How a [`SimDevice`] starts out. Everything has a working default: a formatted card of the usual
/// geometry on a cable-shaped link, with `ARM` refused.
#[derive(Debug, Clone, Copy)]
pub struct SimOptions {
    /// Extents the card holds. Fewer is how a full card is produced.
    pub extents: u32,
    /// Whether the card arrives formatted. A blank one answers `readOnly`/`unformatted` until
    /// `FORMAT` with an all-zero expected identity recovers it.
    pub formatted: bool,
    /// The sparse disk's seed, so a scenario is reproducible.
    pub seed: u64,
    /// The identity the card is formatted with. Two identities is how a client that caches per card
    /// is held to noticing it is looking at a different card.
    pub store: StoreId,
    /// The link the device answers on.
    pub link: Link,
    /// §5's record ceilings. Lowering the control one is how `LIST` pages.
    pub control_ceiling: usize,
    pub stream_ceiling: usize,
    /// Bytes §4's reserve needs when `ARM` is allowed; `None` refuses it, as the board does.
    pub arm_reserve: Option<u64>,
}

impl Default for SimOptions {
    fn default() -> Self {
        SimOptions {
            extents: EXTENTS,
            formatted: true,
            seed: 1,
            store: STORE,
            link: Link::Usb,
            control_ceiling: USB_RECORD_CEILING,
            stream_ceiling: USB_RECORD_CEILING,
            arm_reserve: None,
        }
    }
}

/// The engine, the store and the card, with the card outliving the mount.
pub struct SimDevice {
    card: Card,
    device: Device<Card>,
    options: SimOptions,
    /// Whether the request trace is armed. Kept here because a reboot builds a new [`Device`] and
    /// the hook has to survive it, exactly as a trace of what the client sent should.
    tracing: bool,
}

impl SimDevice {
    /// Format (or leave blank) a card and mount a device on it.
    pub fn boot(options: SimOptions) -> SimDevice {
        let blocks = blocks_for(options.extents);
        let card = if options.formatted {
            Card::formatted(blocks, options.seed, options.store)
        } else {
            Card::blank(blocks, options.seed)
        };
        let device = Device::boot_on(card.clone(), options.link, ceilings(&options));
        SimDevice { card, device, options, tracing: false }
    }

    // --- records ---------------------------------------------------------------

    pub fn on_control(&mut self, record: &[u8]) -> Reaction {
        let link = self.options.link;
        match self.options.arm_reserve {
            Some(reserve) => self.device.on_control_with(link, &mut AllowArm { reserve }, record),
            None => self.device.on_control_with(link, &mut OpenPolicy, record),
        }
    }

    pub fn on_stream(&mut self, record: &[u8]) -> Reaction {
        self.device.on_stream(record)
    }

    pub fn poll(&mut self) -> Reaction {
        self.device.poll()
    }

    // --- the wire and the power ------------------------------------------------

    /// Power was lost and came back. Whatever was never synced is gone; the durable card is the same
    /// card, and the store mounts it again.
    pub fn reboot(&mut self) {
        self.card.reboot();
        self.remount();
    }

    fn remount(&mut self) {
        self.device = Device::boot_on(self.card.clone(), self.options.link, ceilings(&self.options));
        if self.tracing {
            self.device.trace_requests();
        }
    }

    // --- what the card holds ---------------------------------------------------

    pub fn commit_sequence(&self) -> u64 {
        self.device.commit_sequence()
    }

    pub fn store_id(&self) -> StoreId {
        self.device.store_id()
    }

    pub fn catalog(&self) -> Vec<EntryMeta> {
        self.device.catalog()
    }

    pub fn read_object(&self, id: u64, revision: u64) -> Option<Vec<u8>> {
        self.device.read_object(id, revision)
    }

    pub fn seed(&mut self, kind: u16, bytes: &[u8], name: &str) -> EntryMeta {
        self.device.seed(kind, bytes, name)
    }

    pub fn seed_reserved(&mut self, kind: u16, reserve: u64, flags: EntryFlags, name: &str) -> EntryMeta {
        self.device.seed_reserved(kind, reserve, flags, name)
    }

    pub fn seed_retained(&mut self, id: u64, bytes: &[u8], name: &str) -> EntryMeta {
        self.device.seed_retained(id, bytes, name)
    }

    // --- the two test hooks ----------------------------------------------------

    pub fn trace_requests(&mut self) {
        self.tracing = true;
        self.device.trace_requests();
    }

    pub fn take_request_trace(&mut self) -> Vec<TracedRequest> {
        self.device.take_request_trace()
    }

    pub fn stop_answering(&mut self) {
        self.device.stop_answering();
    }
}

fn ceilings(options: &SimOptions) -> Ceilings {
    Ceilings::new(options.control_ceiling, options.stream_ceiling).expect("a link above the protocol floor")
}
