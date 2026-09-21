//! The device the tests talk to: the real protocol engine over the real flat store on a
//! deterministic simulated card, assembled once and driven from two languages.
//!
//! [`Device`] holds one [`Engine`] and one [`FlatStore`] and answers one bounded [`Reaction`] at a
//! time, [`SimDevice`] adds the card the device owns, so a reboot and a media fault are real, and
//! `wasm.rs` puts the same surface behind `wasm-bindgen` for Vitest and the browser. One assembly,
//! shared by every harness that needs a device.
//!
//! Here: the engine, the store, the card, the link ceilings, seeding straight through the store
//! seam, and two named test hooks. Not here: record framing, packet slicing, backpressure and
//! retries, which belong to the adapter on each side — a device that did them twice would be
//! modelling its own transport.
//!
//! Every TypeScript suite runs on `Link::Usb`, because that is the cable the browser has. The BLE
//! halves of the rules that differ per link — the much smaller record ceilings, and the
//! whole-payload rehash a map skips only over USB — are covered by `obc-link`'s own suites.
//!
//! No policy of its own: [`OpenPolicy`] accepts every payload and refuses `ARM`, and [`AllowArm`]
//! is the one alternative.
//!
//! Two test hooks exist, both off by default, because one existing test each cannot be written
//! without them and neither changes what the engine answers:
//!
//! - [`Device::trace_requests`] records `(opcode, request id)` per control record, for the
//!   assertions about what a flow did not send and about request-id uniqueness.
//! - [`Device::stop_answering`] makes the device deaf, for the hung-device timeout test.
//!
//! A device that needs a third hook is a test that should be asking the engine a different
//! question.

use obc_crc::Crc32;
use obc_link::flat::store::Policy;
use obc_link::flat::{
    CancelCause, Ceilings, Channel, Engine, Link, ObjectId as WireObjectId, Reaction as EngineReaction, RequestId,
    Revision as WireRevision, Stall, StreamBuffers, UploadEnd, UploadProgress,
};
use obc_link::flat::{ObjectKind as WireKind, OpenPolicy};
use obc_storage::flat::seam::Store;
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    RideCheckpoint, StoreId, RIDE_RESUME_LEN,
};

mod card;
mod json;
mod sim;

#[cfg(target_arch = "wasm32")]
mod wasm;

pub use card::Card;
pub use json::{catalog_json, trace_json};
pub use sim::{SimDevice, SimOptions};

/// `FLAT_Store_Format.md` §2: the fixed region is 2 MiB and the extent area starts on the block
/// after it.
pub const EXTENT_AREA: u64 = 4_096;
/// §6: one extent, in blocks. A card this size is well under 64 GiB, so §8 gives it the 1 MiB
/// minimum.
pub const EXTENT_BLOCKS: u64 = 2_048;
/// Extents the test card holds. Enough for several objects and small enough to be free.
pub const EXTENTS: u32 = 64;
/// The card every suite runs on.
pub const TOTAL_BLOCKS: u64 = blocks_for(EXTENTS);

/// The card geometry for a given extent count — a smaller card is how a full table is produced.
pub const fn blocks_for(extents: u32) -> u64 {
    EXTENT_AREA + EXTENT_BLOCKS * extents as u64
}

/// §2's two catalog copies, gates included: the blocks a corruption has to reach and a break must
/// leave exactly as it found them.
pub const CATALOG_BLOCKS: core::ops::Range<u64> = 64..1_088;

/// The identity a test card formats with.
pub const STORE: StoreId = StoreId([0x11; 16]);

/// §5.1's BLE control ceiling at the device's preferred 247-byte MTU.
pub const CONTROL_CEILING: usize = 244;
/// A 1 KiB CoC SDU.
pub const STREAM_CEILING: usize = 1_024;
/// §5.2's USB record ceiling, both channels.
pub const USB_RECORD_CEILING: usize = 8_208;

/// The staging buffer the device runs with: small enough that a few-KiB upload crosses it several
/// times, which is the boundary worth exercising.
const STAGE: usize = 1_024;

/// A blank card of the given geometry.
pub fn blank_card(blocks: u64, seed: u64) -> obc_storage::flat::sim::SparseDisk {
    obc_storage::flat::sim::SparseDisk::blank(blocks, seed)
}

/// A card formatted with [`STORE`].
pub fn formatted_card(blocks: u64, seed: u64) -> obc_storage::flat::sim::SparseDisk {
    let disk = blank_card(blocks, seed);
    FlatStore::initialize(&disk, STORE).expect("the test card formats");
    disk
}

/// The CRC-32 the wire and the card both use.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finalize()
}

/// What the device wants done next, with the bytes it produced.
///
/// The engine's own [`EngineReaction`] names a length into a buffer the caller lent it; this one
/// owns its record, because the two adapters that consume it are a Rust pump that collects records
/// and a JavaScript one that copies them into a `Uint8Array` anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// Nothing to do.
    Idle,
    /// Send these bytes on this channel.
    Send { channel: Channel, bytes: Vec<u8> },
    /// §3.1's unanswerable record: close this record stream and emit nothing.
    Close(Channel),
    /// §4 steps 4 and 5: send this on the control channel, drain the link, then reboot.
    SendAndReboot { bytes: Vec<u8> },
}

/// One control record the device was asked to serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TracedRequest {
    pub opcode: u8,
    pub request_id: u32,
}

/// A device with an update path: §4's two hooks answered yes.
///
/// The default [`OpenPolicy`] refuses `ARM`, which is what the board does today. This exists so the
/// success shape has a test, and nothing else about it is a policy: it validates nothing and hands
/// off to nothing.
#[derive(Debug, Clone, Copy)]
pub struct AllowArm {
    /// Bytes §4 step 1 reports the rollback reserve needs.
    pub reserve: u64,
}

impl Policy for AllowArm {
    fn validate_package(&mut self, _package: WireObjectId, _revision: WireRevision) -> Result<u64, u16> {
        Ok(self.reserve)
    }

    fn hand_off(
        &mut self,
        _package: (WireObjectId, WireRevision),
        _reserve: (WireObjectId, WireRevision),
    ) -> Result<(), u16> {
        Ok(())
    }
}

/// One store and one engine over whatever card the caller handed it.
///
/// Everything below is one engine or one store call. The device never loops: a caller that wants
/// the next record calls [`poll`](Device::poll) again, which is what keeps a download from being
/// drained into a vector before the other side gets a turn.
pub struct Device<D: BlockDevice> {
    /// The card's store. Public because every probe a test writes is a question for the store, and
    /// wrapping thirty of them in delegating methods would say nothing.
    pub store: FlatStore<D>,
    engine: Engine<FlatStore<D>, STAGE>,
    out: Vec<u8>,
    link: Link,
    ceilings: Ceilings,
    /// The request trace, when a test asked for one. `None` is the default and costs nothing.
    trace: Option<Vec<TracedRequest>>,
    /// False once a test made the device deaf.
    answering: bool,
}

impl<D: BlockDevice> Device<D> {
    /// Mounts a card and puts an idle engine on a BLE-shaped link.
    pub fn boot(disk: D) -> Self {
        let ceilings = Ceilings::new(CONTROL_CEILING, STREAM_CEILING).expect("a link above the floor");
        Device::boot_on(disk, Link::Ble, ceilings)
    }

    /// The same on a named link of the caller's shape — the cable, where §5.2's ceiling is a
    /// constant of the binding rather than a negotiated radio number.
    pub fn boot_on(disk: D, link: Link, ceilings: Ceilings) -> Self {
        let mut device = Device {
            store: FlatStore::mount(disk),
            engine: Engine::new(),
            out: vec![0; ceilings.stream().max(ceilings.control()) + 64],
            link,
            ceilings,
            trace: None,
            answering: true,
        };
        device.engine.on_link_up(link, &device.store, ceilings);
        device
    }

    /// The ceilings that link came up with.
    pub fn ceilings(&self) -> Ceilings {
        self.ceilings
    }

    // --- records ---------------------------------------------------------------

    /// One whole control record, on the device's own link and under the open policy.
    pub fn on_control(&mut self, record: &[u8]) -> Reaction {
        self.on_control_with(self.link, &mut OpenPolicy, record)
    }

    /// The same from a named link and under a named policy.
    pub fn on_control_with<P: Policy>(&mut self, link: Link, policy: &mut P, record: &[u8]) -> Reaction {
        if let Some(trace) = self.trace.as_mut() {
            // §3's header: the opcode at byte 5, the `RequestId` at bytes 12..16. A record too
            // short to carry either is not a request and is not traced.
            if record.len() >= 16 {
                trace.push(TracedRequest {
                    opcode: record[5],
                    request_id: u32::from_le_bytes([record[12], record[13], record[14], record[15]]),
                });
            }
        }
        if !self.answering {
            return Reaction::Idle;
        }
        let reaction = self.engine.on_control(link, &self.store, policy, record, &mut self.out);
        self.own(reaction)
    }

    /// One whole stream record, on the device's own link and under the open policy.
    pub fn on_stream(&mut self, record: &[u8]) -> Reaction {
        self.on_stream_with(self.link, &mut OpenPolicy, record)
    }

    /// The same from a named link and under a named policy.
    pub fn on_stream_with<P: Policy>(&mut self, link: Link, policy: &mut P, record: &[u8]) -> Reaction {
        if !self.answering {
            return Reaction::Idle;
        }
        let reaction = self.engine.on_stream(link, &self.store, policy, record, &mut self.out);
        self.own(reaction)
    }

    /// One stream record through an adapter-owned write-combining stage: the cable's throughput
    /// seam. `stage` is the whole two-bank arena; the device lends the engine the half it asked for.
    pub fn on_stream_staged(&mut self, link: Link, record: &[u8], stage: &mut [u8]) -> Reaction {
        if !self.answering {
            return Reaction::Idle;
        }
        let bank = self.engine.upload_stage_bank().expect("a staged stream record needs a live upload");
        let half = stage.len() / 2;
        let reaction = self.engine.on_stream_staged(
            link,
            &self.store,
            &mut OpenPolicy,
            StreamBuffers::new(record, &mut self.out),
            bank,
            &mut stage[bank * half..(bank + 1) * half],
        );
        self.own(reaction)
    }

    /// The next record a live transfer owes, on the device's own link.
    pub fn poll(&mut self) -> Reaction {
        self.poll_on(self.link)
    }

    /// The same for a named link.
    pub fn poll_on(&mut self, link: Link) -> Reaction {
        if !self.answering {
            return Reaction::Idle;
        }
        let reaction = self.engine.poll(link, &self.store, &mut self.out);
        self.own(reaction)
    }

    fn own(&self, reaction: EngineReaction) -> Reaction {
        match reaction {
            EngineReaction::Idle => Reaction::Idle,
            EngineReaction::Close(channel) => Reaction::Close(channel),
            EngineReaction::Send { channel, len } => Reaction::Send { channel, bytes: self.out[..len].to_vec() },
            EngineReaction::SendAndReboot { len } => Reaction::SendAndReboot { bytes: self.out[..len].to_vec() },
        }
    }

    // --- links -----------------------------------------------------------------

    /// A link came up (or back up) with its own ceilings.
    pub fn link_up(&mut self, link: Link, ceilings: Ceilings) {
        self.engine.on_link_up(link, &self.store, ceilings);
    }

    /// That link went away, and **only** that: a peer that reconnects calls
    /// [`link_up`](Device::link_up) itself. Bundling the two would make an unplug unobservable.
    pub fn link_down(&mut self, link: Link) {
        self.engine.on_link_lost(link, &self.store);
    }

    // --- what the engine knows -------------------------------------------------

    /// One turn of the stall watchdog, on the caller's own fake clock.
    pub fn watch_stall(&mut self, now_ms: u32) -> Stall {
        self.engine.watch_stall(&self.store, now_ms)
    }

    /// The device drops the live transfer of its own accord (§3.8's other direction).
    pub fn cancel_live(&mut self, cause: CancelCause) -> bool {
        self.engine.cancel_live(&self.store, cause)
    }

    /// True when nothing is live and nothing is owed.
    pub fn is_quiet(&self) -> bool {
        self.engine.is_quiet()
    }

    /// What the live upload has landed so far.
    pub fn live_upload(&self) -> Option<UploadProgress> {
        self.engine.live_upload()
    }

    /// Whether an exact upload owns the engine.
    pub fn upload_matches(&self, link: Link, request: RequestId, kind: WireKind) -> bool {
        self.engine.upload_matches(link, request, kind)
    }

    /// The verdict on the last upload, taken.
    pub fn take_upload_end(&mut self) -> Option<(WireKind, UploadEnd)> {
        self.engine.take_upload_end()
    }

    // --- what the card holds ---------------------------------------------------

    /// The catalog commit sequence.
    pub fn commit_sequence(&self) -> u64 {
        self.store.sequence()
    }

    /// The card's identity.
    pub fn store_id(&self) -> StoreId {
        self.store.store_id()
    }

    /// Every entry, in catalog order — what a complete `LIST` would page out.
    pub fn catalog(&self) -> Vec<EntryMeta> {
        Store::entries(&self.store).collect()
    }

    /// The bytes behind one entry, read through the store. `revision` of zero means the head.
    pub fn read_object(&self, id: u64, revision: u64) -> Option<Vec<u8>> {
        let wanted = (revision != 0).then_some(Revision(revision));
        let handle = Store::open(&self.store, ObjectId(id), wanted).ok()?;
        let len = self.store.handle_len(&handle).unwrap_or(0) as usize;
        let mut bytes = vec![0u8; len];
        let mut at = 0;
        while at < len {
            match Store::read(&self.store, &handle, at as u64, &mut bytes[at..]) {
                Ok(0) => break,
                Ok(read) => at += read,
                Err(_) => {
                    self.store.close(handle);
                    return None;
                }
            }
        }
        self.store.close(handle);
        bytes.truncate(at);
        Some(bytes)
    }

    // --- seeding ---------------------------------------------------------------

    /// Publishes an object straight through the store seam: how a test gets a card with something
    /// on it without spending a transfer on it. The store assigns the id.
    pub fn seed(&mut self, kind: u16, bytes: &[u8], name: &str) -> EntryMeta {
        let mut allocation = Store::allocate(&self.store, bytes.len() as u64).expect("the seed allocates");
        Store::write(&self.store, &mut allocation, bytes).expect("the seed writes");
        let meta = EntryMeta {
            added_at_utc: 0,
            id: FlatStore::next_object_id(&self.store),
            revision: Revision(1),
            kind: seam_kind(kind),
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: crc32(bytes),
            name: DisplayName::new(name).expect("a seed name"),
        };
        Store::commit(&self.store, &[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
            .expect("the seed commits");
        meta
    }

    /// Publishes a further revision of an object already on the card and keeps the previous one as
    /// `RETAINED` — §5.3's non-head revision the store keeps alive on purpose.
    ///
    /// Both entries go in one commit, which is the only shape the catalog admits, and the id comes
    /// from the card rather than from the caller. It is the one catalog state no opcode produces,
    /// which is why a test that needs one needs this.
    pub fn seed_retained(&mut self, id: u64, bytes: &[u8], name: &str) -> EntryMeta {
        let previous = Store::entries(&self.store)
            .find(|meta| meta.id == ObjectId(id) && !meta.flags.has(EntryFlags::RETAINED))
            .expect("the object to retain is on the card");
        let mut allocation = Store::allocate(&self.store, bytes.len() as u64).expect("the revision allocates");
        Store::write(&self.store, &mut allocation, bytes).expect("the revision writes");
        let head = EntryMeta {
            revision: Revision(previous.revision.0 + 1),
            payload_len: bytes.len() as u64,
            payload_crc: crc32(bytes),
            name: DisplayName::new(name).expect("a seed name"),
            ..previous
        };
        let flags = EntryFlags::decode(previous.flags.bits() | EntryFlags::RETAINED.bits()).expect("§5.3 flags");
        let retained = EntryMeta { flags, ..previous };
        Store::commit(
            &self.store,
            &[
                Mutation::Put { meta: head, source: PutSource::Fresh(allocation) },
                Mutation::Put { meta: retained, source: PutSource::Amend },
            ],
        )
        .expect("one commit publishes the head and retains the previous revision");
        head
    }

    /// Publishes an entry over a reserve with no bytes behind it: a ride mid-recording, or an
    /// update's rollback reserve. A `GET` of either is refused, which is the point of having one.
    pub fn seed_reserved(&mut self, kind: u16, reserve: u64, flags: EntryFlags, name: &str) -> EntryMeta {
        let allocation = Store::allocate(&self.store, reserve).expect("the reserve allocates");
        let meta = EntryMeta {
            added_at_utc: 0,
            id: FlatStore::next_object_id(&self.store),
            revision: Revision(1),
            kind: seam_kind(kind),
            flags,
            payload_len: 0,
            payload_crc: 0,
            name: DisplayName::new(name).expect("a seed name"),
        };
        Store::commit(&self.store, &[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
            .expect("the reserve commits");
        meta
    }

    /// The exact device-owned FS8 path: start a `RECORDING` reserve, journal the final bytes, then
    /// publish those same extents by clearing `RECORDING` in one amend commit.
    pub fn finish_recording(&mut self, bytes: &[u8], name: &str) -> EntryMeta {
        let started = self.seed_reserved(ObjectKind::Ride as u16, 1024 * 1024, EntryFlags::RECORDING, "");
        let resume = [0u8; RIDE_RESUME_LEN];
        Store::journal(
            &self.store,
            RideCheckpoint {
                id: started.id,
                revision: started.revision,
                append: bytes,
                payload_crc: crc32(bytes),
                resume: &resume,
            },
        )
        .expect("the final ride bytes journal");
        let meta = EntryMeta {
            added_at_utc: 0,
            id: started.id,
            revision: started.revision,
            kind: ObjectKind::Ride,
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: crc32(bytes),
            name: DisplayName::new(name).expect("a ride name"),
        };
        Store::commit(&self.store, &[Mutation::Put { meta, source: PutSource::Amend }])
            .expect("one final commit publishes the ride");
        meta
    }

    // --- the two test hooks ----------------------------------------------------

    /// Start recording `(opcode, request id)` per control record. Off by default.
    pub fn trace_requests(&mut self) {
        self.trace.get_or_insert_with(Vec::new);
    }

    /// Every traced request since the last call, taken.
    pub fn take_request_trace(&mut self) -> Vec<TracedRequest> {
        self.trace.as_mut().map(core::mem::take).unwrap_or_default()
    }

    /// Stop answering anything: an enumerated device that has hung. Records still arrive and are
    /// still traced; nothing comes back.
    pub fn stop_answering(&mut self) {
        self.answering = false;
    }
}

/// The two kind tables are the same table, stated once on each side of the seam.
fn seam_kind(kind: u16) -> ObjectKind {
    ObjectKind::decode(kind).expect("a kind §3.1 names")
}
