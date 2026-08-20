//! The host harness the protocol-v4 suites run on: a real flat store over a deterministic sparse
//! card, the engine driven exactly as an adapter drives it, and a client that builds request bytes
//! by hand.
//!
//! Building requests here rather than through the codec is deliberate. The device never encodes a
//! request, so `obc-link` has no encoder for one; writing the bytes at the offsets
//! `FLAT_Store_Protocol.md` §3 states keeps the tests honest about what a phone would actually send.

#![allow(dead_code)]

use obc_link::flat::store::Policy;
use obc_link::flat::wire::{flags, HEADER_LEN, STREAM_HEADER_LEN};
use obc_link::flat::{
    CancelCause, Ceilings, Channel, Engine, Link, ObjectKind, OpenPolicy, Reaction, RequestId, UploadStage,
};
use obc_storage::flat::sim::{FaultOnce, SparseDisk};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, PutSource, Revision, StoreId,
};

/// `FLAT_Store_Format.md` §2: the fixed region is 2 MiB and the extent area starts on the block
/// after it.
pub const EXTENT_AREA: u64 = 4_096;
/// §6: one extent, in blocks. A card this size is well under 64 GiB, so §8 gives it the 1 MiB
/// minimum — the harness never has to know the size is card-scaled, because everything it drives is
/// byte-addressed at the seam.
pub const EXTENT_BLOCKS: u64 = 2_048;
/// Extents the test card holds. Enough for several objects and small enough to be free.
pub const EXTENTS: u32 = 64;
/// The card every suite runs on.
pub const TOTAL_BLOCKS: u64 = EXTENT_AREA + EXTENT_BLOCKS * EXTENTS as u64;
/// The catalog's two copies, gates included (§2), which is the byte image a break must not change.
pub const CATALOG_BLOCKS: core::ops::Range<u64> = 64..1_088;

/// The identity the harness formats with.
pub const STORE: StoreId = StoreId([0x11; 16]);

/// §5.1's BLE control ceiling at the device's preferred 247-byte MTU.
pub const CONTROL_CEILING: usize = 244;
/// A 1 KiB CoC SDU.
pub const STREAM_CEILING: usize = 1_024;

/// The staging buffer the suites run with: small enough that a few-KiB upload crosses it several
/// times, which is the boundary worth exercising.
const STAGE: usize = 1_024;

/// A device over a plain card.
pub type Plain<'a> = Device<&'a SparseDisk>;
/// A device over a card that refuses one media operation and then behaves.
pub type Faulty<'a> = Device<&'a FaultOnce<&'a SparseDisk>>;

/// A blank card of the harness geometry.
pub fn blank_card(seed: u64) -> SparseDisk {
    SparseDisk::blank(TOTAL_BLOCKS, seed)
}

/// A card this harness has formatted.
pub fn formatted_card(seed: u64) -> SparseDisk {
    let disk = SparseDisk::blank(TOTAL_BLOCKS, seed);
    FlatStore::initialize(&disk, STORE).expect("the harness card formats");
    disk
}

/// Deterministic payload bytes, so a stored CRC means something.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 7 + 11) as u8).collect()
}

/// The CRC-32 the wire and the card both use.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = obc_crc::Crc32::new();
    crc.update(bytes);
    crc.finalize()
}

/// The catalog's byte image: what a break must leave exactly as it found it.
pub fn catalog_image(disk: &SparseDisk) -> u32 {
    let mut crc = obc_crc::Crc32::new();
    for lba in CATALOG_BLOCKS {
        crc.update(&disk.block(lba));
    }
    crc.finalize()
}

/// One record the client sent, in the order the engine answered it.
#[derive(Debug, Default)]
pub struct Wire {
    pub control: Vec<Vec<u8>>,
    pub stream: Vec<Vec<u8>>,
    pub closed: Option<Channel>,
    pub reboot: bool,
}

impl Wire {
    /// The one control record this exchange produced.
    pub fn answer(&self) -> &[u8] {
        assert_eq!(self.control.len(), 1, "expected exactly one control answer, got {:?}", self.control.len());
        &self.control[0]
    }

    /// Every stream payload, concatenated: what a client would have assembled.
    pub fn payload(&self) -> Vec<u8> {
        self.stream.iter().flat_map(|record| record[STREAM_HEADER_LEN..].to_vec()).collect()
    }
}

/// The device: one store and one engine, over whatever card the suite handed it.
pub struct Device<D: BlockDevice> {
    pub store: FlatStore<D>,
    engine: Engine<FlatStore<D>, STAGE>,
    out: Vec<u8>,
    /// What [`Device::link_lost`] brings the radio back up with. A link that is down cannot be
    /// served at all now, so a helper that models a *break* has to model the reconnect too.
    ceilings: Ceilings,
}

/// Mounts a card and puts an idle engine on a BLE-shaped link.
pub fn boot<D: BlockDevice>(disk: D) -> Device<D> {
    boot_on(disk, Ceilings::new(CONTROL_CEILING, STREAM_CEILING).expect("a link above the floor"))
}

/// The same on a link of the caller's shape — a USB one, where §5.2's ceiling is a constant of the
/// binding and a stream record is whole stages wide rather than one radio SDU.
pub fn boot_on<D: BlockDevice>(disk: D, ceilings: Ceilings) -> Device<D> {
    let mut device = Device {
        store: FlatStore::mount(disk),
        out: vec![0; ceilings.stream().max(ceilings.control()) + STREAM_HEADER_LEN],
        engine: Engine::new(),
        ceilings,
    };
    // Every test that does not care which wire it is on is on the radio, which is what the suite
    // meant before links had identities. `link_up` is what a test that *does* care calls.
    device.engine.on_link_up(Link::Ble, &device.store, ceilings);
    device
}

impl<D: BlockDevice> Device<D> {
    /// Hands the engine one control record and pumps it until it is quiet. The device has no kind
    /// validators and no update path, which is what a board without FS7 and FS9 runs.
    pub fn control(&mut self, record: &[u8]) -> Wire {
        self.control_with(record, &mut OpenPolicy)
    }

    /// Bring a link up (or back up) with its own ceilings — the two-link cases.
    pub fn link_up(&mut self, link: Link, ceilings: Ceilings) {
        self.engine.on_link_up(link, &self.store, ceilings);
    }

    /// One control record from a named link.
    pub fn control_on(&mut self, link: Link, record: &[u8]) -> Wire {
        let first = self.engine.on_control(link, &self.store, &mut OpenPolicy, record, &mut self.out);
        self.drive_on(link, first, usize::MAX)
    }

    /// One stream record from a named link.
    pub fn stream_on(&mut self, link: Link, record: &[u8]) -> Wire {
        let first = self.engine.on_stream(link, &self.store, &mut OpenPolicy, record, &mut self.out);
        self.drive_on(link, first, usize::MAX)
    }

    /// One stream record through an adapter-owned write-combining stage.
    pub fn stream_on_staged(&mut self, link: Link, record: &[u8], stage: &mut [u8]) -> Wire {
        let bank = self.engine.upload_stage_bank().expect("staged stream owns an upload");
        let half = stage.len() / 2;
        let first = self.engine.on_stream_staged(
            link,
            &self.store,
            &mut OpenPolicy,
            record,
            &mut self.out,
            UploadStage::new(bank, &mut stage[bank * half..(bank + 1) * half]),
        );
        self.drive_on(link, first, usize::MAX)
    }

    /// Pump a named link once — what an adapter does until it is told there is nothing to do.
    pub fn pump_on(&mut self, link: Link) -> Wire {
        let first = self.engine.poll(link, &self.store, &mut self.out);
        self.drive_on(link, first, usize::MAX)
    }

    /// That link went away.
    pub fn link_lost_on(&mut self, link: Link) {
        self.engine.on_link_lost(link, &self.store);
    }

    /// The same on a device whose policy hooks are filled in.
    pub fn control_with<P: Policy>(&mut self, record: &[u8], policy: &mut P) -> Wire {
        let first = self.engine.on_control(Link::Ble, &self.store, policy, record, &mut self.out);
        self.drive(first, usize::MAX)
    }

    /// One control record, pumped at most `budget` times: a link that goes quiet part-way through a
    /// download, which is the only way to catch one in flight.
    pub fn control_upto(&mut self, record: &[u8], budget: usize) -> Wire {
        self.control_with_upto(record, &mut OpenPolicy, budget)
    }

    /// Both at once, for a flow that arms an update and is then cut.
    pub fn control_with_upto<P: Policy>(&mut self, record: &[u8], policy: &mut P, budget: usize) -> Wire {
        let first = self.engine.on_control(Link::Ble, &self.store, policy, record, &mut self.out);
        self.drive(first, budget)
    }

    /// Pumps a live transfer until it goes quiet.
    pub fn pump(&mut self) -> Wire {
        let first = self.engine.poll(Link::Ble, &self.store, &mut self.out);
        self.drive(first, usize::MAX)
    }

    /// Pumps exactly one record out of it.
    pub fn pump_once(&mut self) -> Wire {
        let first = self.engine.poll(Link::Ble, &self.store, &mut self.out);
        self.drive(first, 1)
    }

    /// One stream record.
    pub fn stream(&mut self, record: &[u8]) -> Wire {
        let first = self.engine.on_stream(Link::Ble, &self.store, &mut OpenPolicy, record, &mut self.out);
        self.drive(first, usize::MAX)
    }

    /// The catalog commit sequence.
    pub fn commit_sequence(&self) -> u64 {
        self.store.sequence()
    }

    /// The link went away.
    /// **The radio link broke and the client came back** — a break, in the sense the break matrix
    /// means it: the transfer is released with nobody to answer, and the next thing the peer does is
    /// reconnect and retry. Both halves are here because a link that is down is now genuinely
    /// unserved (`on_control` answers `Idle`), so a helper that only tore down would leave every
    /// following statement in those tests talking to a wire nobody is on.
    ///
    /// [`link_lost_on`](Device::link_lost_on) is the un-reconnected half, for the two-link tests
    /// that care about the difference.
    pub fn link_lost(&mut self) {
        self.engine.on_link_lost(Link::Ble, &self.store);
        self.engine.on_link_up(Link::Ble, &self.store, self.ceilings);
    }

    /// The device drops the live transfer of its own accord (§3.8's other direction).
    pub fn cancel_live(&mut self, cause: CancelCause) -> bool {
        self.engine.cancel_live(&self.store, cause)
    }

    /// True when nothing is live and nothing is owed.
    pub fn is_quiet(&self) -> bool {
        self.engine.is_quiet()
    }

    /// What the live upload has landed so far — the device-side progress report.
    pub fn live_upload(&self) -> Option<obc_link::flat::UploadProgress> {
        self.engine.live_upload()
    }

    /// Whether an exact upload owns the engine, used by adapter-resource admission tests.
    pub fn upload_matches(&self, link: Link, request: RequestId, kind: ObjectKind) -> bool {
        self.engine.upload_matches(link, request, kind)
    }

    /// The verdict on the last upload, taken.
    pub fn take_upload_end(&mut self) -> Option<(ObjectKind, obc_link::flat::UploadEnd)> {
        self.engine.take_upload_end()
    }

    fn drive(&mut self, first: Reaction, budget: usize) -> Wire {
        self.drive_on(Link::Ble, first, budget)
    }

    fn drive_on(&mut self, link: Link, first: Reaction, budget: usize) -> Wire {
        let mut wire = Wire::default();
        let mut reaction = first;
        let mut sent = 0;
        loop {
            match reaction {
                Reaction::Idle => break,
                Reaction::Close(channel) => {
                    wire.closed = Some(channel);
                    break;
                }
                Reaction::Send { channel, len } => {
                    let record = self.out[..len].to_vec();
                    match channel {
                        Channel::Control => wire.control.push(record),
                        Channel::Stream => wire.stream.push(record),
                    }
                }
                Reaction::SendAndReboot { len } => {
                    wire.control.push(self.out[..len].to_vec());
                    wire.reboot = true;
                    break;
                }
            }
            sent += 1;
            if sent >= budget {
                break;
            }
            reaction = self.engine.poll(link, &self.store, &mut self.out);
        }
        wire
    }

    /// Publishes an object straight through the seam, which is how a suite gets a card with
    /// something on it without spending a transfer on it.
    pub fn seed(&mut self, kind: ObjectKind, bytes: &[u8], name: &str) -> (u64, u64) {
        let id = FlatStore::next_object_id(&self.store);
        let mut allocation = Store::allocate(&self.store, bytes.len() as u64).expect("the seed allocates");
        Store::write(&self.store, &mut allocation, bytes).expect("the seed writes");
        let meta = EntryMeta {
            id,
            revision: Revision(1),
            kind: seam_kind(kind),
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: crc32(bytes),
            name: DisplayName::new(name).expect("a seed name"),
        };
        Store::commit(&self.store, &[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
            .expect("the seed commits");
        (id.0, 1)
    }

    /// Publishes the one entry a client may never touch: a ride, mid-recording, over a reserve.
    pub fn seed_recording(&mut self, reserve: u64) -> (u64, u64) {
        let id = FlatStore::next_object_id(&self.store);
        let allocation = Store::allocate(&self.store, reserve).expect("the ride reserves");
        let meta = EntryMeta {
            id,
            revision: Revision(1),
            kind: obc_storage::flat::ObjectKind::Ride,
            flags: EntryFlags::RECORDING,
            payload_len: 0,
            payload_crc: 0,
            name: DisplayName::default(),
        };
        Store::commit(&self.store, &[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
            .expect("the ride starts");
        (id.0, 1)
    }

    /// Takes a reservation row out from under the engine, which is how a full table is produced.
    pub fn hog(&mut self, bytes: u64) -> obc_storage::flat::Allocation {
        Store::allocate(&self.store, bytes).expect("a row was free")
    }

    /// Gives one back.
    pub fn release(&mut self, allocation: obc_storage::flat::Allocation) {
        FlatStore::cancel(&self.store, allocation);
    }

    /// Free extents, which is what a leaked reservation or a leaked hold shows up in.
    pub fn free_extents(&self) -> u32 {
        self.store.free_extents()
    }

    /// The catalog entry for one id, straight from the store.
    pub fn entry(&self, id: u64) -> Option<EntryMeta> {
        Store::entries(&self.store).find(|meta| meta.id == ObjectId(id) && !meta.flags.has(EntryFlags::RETAINED))
    }

    /// Every entry, in catalog order.
    pub fn entries(&self) -> Vec<EntryMeta> {
        Store::entries(&self.store).collect()
    }

    /// Removes every entry of one `ObjectId` through the seam, and reports the extents that came
    /// back with them. A hold the engine failed to close keeps them out of the allocator, so this is
    /// how a leaked hold is caught. Both entries go in one commit, because §5.3 has no state in which
    /// a retained revision is an object's only entry.
    pub fn remove_and_measure(&mut self, id: u64) -> u32 {
        let before = self.free_extents();
        let batch: Vec<Mutation> = Store::entries(&self.store)
            .filter(|meta| meta.id == ObjectId(id))
            .map(|meta| Mutation::Remove { id: meta.id, revision: meta.revision })
            .collect();
        assert!(!batch.is_empty(), "the probe names an entry that is not there");
        Store::commit(&self.store, &batch).expect("the probe removes");
        self.free_extents() - before
    }
}

fn seam_kind(kind: ObjectKind) -> obc_storage::flat::ObjectKind {
    obc_storage::flat::ObjectKind::decode(kind.value()).expect("the two tables are the same table")
}

use obc_storage::flat::seam::Store;

/// Request bytes, written at the offsets `FLAT_Store_Protocol.md` §3 states.
pub mod client {
    use super::*;

    fn frame(opcode: u8, request: u32, body: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; HEADER_LEN + body.len()];
        record[0..4].copy_from_slice(b"OBC4");
        record[4] = 4;
        record[5] = opcode;
        record[8..10].copy_from_slice(&(body.len() as u16).to_le_bytes());
        record[12..16].copy_from_slice(&request.to_le_bytes());
        record[HEADER_LEN..].copy_from_slice(body);
        record
    }

    /// §3.3, first page.
    pub fn list(request: u32, kind: Option<u16>) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..2].copy_from_slice(&kind.unwrap_or(0).to_le_bytes());
        frame(0x01, request, &body)
    }

    /// §3.3, a page resuming after a `(ObjectId, Revision)` pair.
    pub fn list_from(request: u32, kind: Option<u16>, cursor: (u64, u64), sequence: u64) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..2].copy_from_slice(&kind.unwrap_or(0).to_le_bytes());
        body[2..4].copy_from_slice(&1u16.to_le_bytes());
        body[8..16].copy_from_slice(&cursor.0.to_le_bytes());
        body[16..24].copy_from_slice(&cursor.1.to_le_bytes());
        body[24..32].copy_from_slice(&sequence.to_le_bytes());
        frame(0x01, request, &body)
    }

    /// §3.4.
    pub fn status(request: u32, id: u64, revision: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&revision.to_le_bytes());
        frame(0x02, request, &body)
    }

    /// §3.5.
    pub fn get(request: u32, id: u64, revision: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&revision.to_le_bytes());
        frame(0x03, request, &body)
    }

    /// §3.6.
    pub fn put(request: u32, id: u64, expected: u64, bytes: &[u8], kind: u16, retain: bool, name: &str) -> Vec<u8> {
        let mut body = vec![0u8; 84];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        body[16..24].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        body[24..28].copy_from_slice(&crc32(bytes).to_le_bytes());
        body[28..30].copy_from_slice(&kind.to_le_bytes());
        body[30..32].copy_from_slice(&u16::from(retain).to_le_bytes());
        body[32] = name.len() as u8;
        body[36..36 + name.len()].copy_from_slice(name.as_bytes());
        frame(0x04, request, &body)
    }

    /// §3.7.
    pub fn remove(request: u32, id: u64, expected: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        frame(0x05, request, &body)
    }

    /// §3.8.
    pub fn cancel(request: u32, transfer: u32) -> Vec<u8> {
        frame(0x06, request, &transfer.to_le_bytes())
    }

    /// §4.
    pub fn arm(request: u32, package: u64, expected: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&package.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        frame(0x07, request, &body)
    }

    /// §3.10. The expected identity is the destructive compare-and-swap; replacement starts the
    /// new store era and must be non-zero and different.
    pub fn format(request: u32, expected: [u8; 16], replacement: [u8; 16]) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..16].copy_from_slice(&expected);
        body[16..32].copy_from_slice(&replacement);
        frame(0x08, request, &body)
    }

    /// §3.8's stream record.
    pub fn stream(transfer: u32, offset: u64, bytes: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; STREAM_HEADER_LEN + bytes.len()];
        record[0..4].copy_from_slice(&transfer.to_le_bytes());
        record[4..12].copy_from_slice(&offset.to_le_bytes());
        record[12..14].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
        record[STREAM_HEADER_LEN..].copy_from_slice(bytes);
        record
    }

    /// One upload's payload cut into stream records of `chunk` bytes.
    pub fn stream_all(transfer: u32, bytes: &[u8], chunk: usize) -> Vec<Vec<u8>> {
        bytes.chunks(chunk).enumerate().map(|(index, part)| stream(transfer, (index * chunk) as u64, part)).collect()
    }
}

/// A decoded control response, as a client reads it.
#[derive(Debug, Clone)]
pub struct Answer {
    pub opcode: u8,
    pub flags: u16,
    pub request: u32,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn of(record: &[u8]) -> Self {
        assert_eq!(&record[0..4], b"OBC4", "an answer that is not a v4 frame");
        assert_eq!(record[4], 4);
        let declared = u16::from_le_bytes([record[8], record[9]]) as usize;
        assert_eq!(record.len(), HEADER_LEN + declared, "the declared length is not the record's");
        Answer {
            opcode: record[5],
            flags: u16::from_le_bytes([record[6], record[7]]),
            request: u32::from_le_bytes([record[12], record[13], record[14], record[15]]),
            body: record[HEADER_LEN..].to_vec(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.flags & flags::ERROR != 0
    }

    pub fn has_more(&self) -> bool {
        self.flags & flags::MORE != 0
    }

    /// `(code, detail, context)` of an error response.
    pub fn error(&self) -> (u16, u16, u64) {
        assert!(self.is_error(), "not an error response: {self:?}");
        assert_eq!(self.body.len(), 16);
        (
            u16::from_le_bytes([self.body[0], self.body[1]]),
            u16::from_le_bytes([self.body[2], self.body[3]]),
            u64::from_le_bytes(self.body[4..12].try_into().unwrap()),
        )
    }

    pub fn u64_at(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.body[at..at + 8].try_into().unwrap())
    }

    /// One body byte — §3.8's `CANCEL` answer is exactly one.
    pub fn byte_at(&self, at: usize) -> u8 {
        self.body[at]
    }

    pub fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.body[at..at + 4].try_into().unwrap())
    }
}
