//! The host harness these suites run on: the shared device from `obc-flat-device` — a real flat
//! store over a deterministic sparse card with the engine on top — plus the pump, the probes and the
//! hand-written client this suite drives it with.
//!
//! The device is the assembly the browser's tests run on too, so a behaviour proved here is proved
//! against the same engine and the same card. The harness is what only a Rust test wants: a pump
//! that collects every record of one exchange into a [`Wire`], store probes, and request bytes
//! written at the offsets `FLAT_Store_Protocol.md` states. The device never encodes a request, so
//! writing those bytes by hand keeps the tests honest about what a phone would send.

#![allow(dead_code)]

use core::ops::{Deref, DerefMut};

use obc_flat_device::Reaction;
use obc_link::flat::store::Policy;
use obc_link::flat::wire::{flags, HEADER_LEN, STREAM_HEADER_LEN};
use obc_link::flat::{Ceilings, Channel, Link, ObjectKind, OpenPolicy};
use obc_storage::flat::sim::SparseDisk;
use obc_storage::flat::{BlockDevice, EntryFlags, EntryMeta, ObjectId};

use obc_flat_device::{CATALOG_BLOCKS, TOTAL_BLOCKS};
// The two test targets compile this module separately and name different halves of it, which is
// what the `dead_code` allow above is for.
#[allow(unused_imports)]
pub use obc_flat_device::{crc32, STORE};

pub type Plain<'a> = Device<&'a SparseDisk>;
/// A device over a card that refuses one media operation and then behaves.
pub type Faulty<'a> = Device<&'a obc_storage::flat::sim::FaultOnce<&'a SparseDisk>>;

pub fn blank_card(seed: u64) -> SparseDisk {
    obc_flat_device::blank_card(TOTAL_BLOCKS, seed)
}

pub fn formatted_card(seed: u64) -> SparseDisk {
    obc_flat_device::formatted_card(TOTAL_BLOCKS, seed)
}

/// Deterministic payload bytes, so a stored CRC means something.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 7 + 11) as u8).collect()
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

/// The shared device, plus this suite's pump and probes.
///
/// Everything the device itself offers — the store, the engine's own questions, the link and the
/// seeding — reaches through [`Deref`], so `device.store`, `device.is_quiet()` and
/// `device.watch_stall(..)` are the device's and are not restated here.
pub struct Device<D: BlockDevice> {
    inner: obc_flat_device::Device<D>,
}

impl<D: BlockDevice> Deref for Device<D> {
    type Target = obc_flat_device::Device<D>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<D: BlockDevice> DerefMut for Device<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Mounts a card and puts an idle engine on a BLE-shaped link.
pub fn boot<D: BlockDevice>(disk: D) -> Device<D> {
    Device { inner: obc_flat_device::Device::boot(disk) }
}

/// The same on a link of the caller's shape — a USB one, whose ceiling is a constant of the binding
/// and whose stream record is whole stages wide rather than one radio SDU.
pub fn boot_on<D: BlockDevice>(disk: D, ceilings: Ceilings) -> Device<D> {
    // Every test that does not care which wire it is on is on the radio. A test that does care
    // calls `link_up`.
    Device { inner: obc_flat_device::Device::boot_on(disk, Link::Ble, ceilings) }
}

impl<D: BlockDevice> Device<D> {
    /// Hands the engine one control record and pumps it until it is quiet. The device has no kind
    /// validators and no update path.
    pub fn control(&mut self, record: &[u8]) -> Wire {
        self.control_with(record, &mut OpenPolicy)
    }

    pub fn control_on(&mut self, link: Link, record: &[u8]) -> Wire {
        let first = self.inner.on_control_with(link, &mut OpenPolicy, record);
        self.drive_on(link, first, usize::MAX)
    }

    pub fn stream_on(&mut self, link: Link, record: &[u8]) -> Wire {
        let first = self.inner.on_stream_with(link, &mut OpenPolicy, record);
        self.drive_on(link, first, usize::MAX)
    }

    /// One stream record through an adapter-owned write-combining stage.
    pub fn stream_on_staged(&mut self, link: Link, record: &[u8], stage: &mut [u8]) -> Wire {
        let first = self.inner.on_stream_staged(link, record, stage);
        self.drive_on(link, first, usize::MAX)
    }

    /// Pump a named link once.
    pub fn pump_on(&mut self, link: Link) -> Wire {
        let first = self.inner.poll_on(link);
        self.drive_on(link, first, usize::MAX)
    }

    pub fn link_lost_on(&mut self, link: Link) {
        self.inner.link_down(link);
    }

    /// The same on a device whose policy hooks are filled in.
    pub fn control_with<P: Policy>(&mut self, record: &[u8], policy: &mut P) -> Wire {
        let first = self.inner.on_control_with(Link::Ble, policy, record);
        self.drive(first, usize::MAX)
    }

    /// One control record, pumped at most `budget` times: a link that goes quiet part-way through a
    /// download, which is the only way to catch one in flight.
    pub fn control_upto(&mut self, record: &[u8], budget: usize) -> Wire {
        self.control_with_upto(record, &mut OpenPolicy, budget)
    }

    /// Both at once, for a flow that arms an update and is then cut.
    pub fn control_with_upto<P: Policy>(&mut self, record: &[u8], policy: &mut P, budget: usize) -> Wire {
        let first = self.inner.on_control_with(Link::Ble, policy, record);
        self.drive(first, budget)
    }

    /// Pumps a live transfer until it goes quiet.
    pub fn pump(&mut self) -> Wire {
        let first = self.inner.poll();
        self.drive(first, usize::MAX)
    }

    /// Pumps exactly one record out of it.
    pub fn pump_once(&mut self) -> Wire {
        let first = self.inner.poll();
        self.drive(first, 1)
    }

    pub fn stream(&mut self, record: &[u8]) -> Wire {
        let first = self.inner.on_stream(record);
        self.drive(first, usize::MAX)
    }

    /// The radio link broke and the client came back: the transfer is released with nobody to
    /// answer, and the peer then reconnects and retries. Both halves are here because a link that is
    /// down is unserved, so tearing down alone would leave the test talking to a wire nobody is on.
    ///
    /// [`link_down`](obc_flat_device::Device::link_down) is the un-reconnected half.
    pub fn link_lost(&mut self) {
        let ceilings = self.inner.ceilings();
        self.inner.link_down(Link::Ble);
        self.inner.link_up(Link::Ble, ceilings);
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
                Reaction::Send { channel, bytes } => match channel {
                    Channel::Control => wire.control.push(bytes),
                    Channel::Stream => wire.stream.push(bytes),
                },
                Reaction::SendAndReboot { bytes } => {
                    wire.control.push(bytes);
                    wire.reboot = true;
                    break;
                }
            }
            sent += 1;
            if sent >= budget {
                break;
            }
            reaction = self.inner.poll_on(link);
        }
        wire
    }

    /// Publishes an object straight through the seam, which is how a suite gets a card with
    /// something on it without spending a transfer on it.
    pub fn seed(&mut self, kind: ObjectKind, bytes: &[u8], name: &str) -> (u64, u64) {
        key(self.inner.seed(kind.value(), bytes, name))
    }

    /// Publishes the one entry a client may never touch: a ride, mid-recording, over a reserve.
    pub fn seed_recording(&mut self, reserve: u64) -> (u64, u64) {
        key(self.inner.seed_reserved(ObjectKind::Ride.value(), reserve, EntryFlags::RECORDING, ""))
    }

    /// The device-owned ride path: start a `RECORDING` reserve, journal the final bytes, then
    /// publish those same extents by clearing `RECORDING` in one amend commit.
    pub fn finish_recording(&mut self, bytes: &[u8], name: &str) -> (u64, u64) {
        key(self.inner.finish_recording(bytes, name))
    }

    /// Takes a reservation row out from under the engine, which is how a full table is produced.
    pub fn hog(&mut self, bytes: u64) -> obc_storage::flat::Allocation {
        Store::allocate(&self.store, bytes).expect("a row was free")
    }

    pub fn release(&mut self, allocation: obc_storage::flat::Allocation) {
        obc_storage::flat::FlatStore::cancel(&self.store, allocation);
    }

    /// Free extents, which is what a leaked reservation or a leaked hold shows up in.
    pub fn free_extents(&self) -> u32 {
        self.store.free_extents()
    }

    /// The catalog entry for one id, straight from the store.
    pub fn entry(&self, id: u64) -> Option<EntryMeta> {
        Store::entries(&self.store).find(|meta| meta.id == ObjectId(id) && !meta.flags.has(EntryFlags::RETAINED))
    }

    pub fn entries(&self) -> Vec<EntryMeta> {
        self.inner.catalog()
    }

    /// Removes every entry of one `ObjectId` through the seam, and reports the extents that came
    /// back with them. A hold the engine failed to close keeps them out of the allocator, so this is
    /// how a leaked hold is caught. Both entries go in one commit, because a retained revision is
    /// never an object's only entry.
    pub fn remove_and_measure(&mut self, id: u64) -> u32 {
        let before = self.free_extents();
        let batch: Vec<obc_storage::flat::Mutation> = Store::entries(&self.store)
            .filter(|meta| meta.id == ObjectId(id))
            .map(|meta| obc_storage::flat::Mutation::Remove { id: meta.id, revision: meta.revision })
            .collect();
        assert!(!batch.is_empty(), "the probe names an entry that is not there");
        Store::commit(&self.store, &batch).expect("the probe removes");
        self.free_extents() - before
    }
}

/// The `(id, revision)` pair the suites name an entry by.
fn key(meta: EntryMeta) -> (u64, u64) {
    (meta.id.0, meta.revision.0)
}

/// A [`ByteSink`] that keeps what was written: what a host hands an exporter when the file is going
/// to a phone rather than to a card.
#[derive(Default)]
pub struct VecSink(pub Vec<u8>);

impl obc_formats::io::ByteSink for VecSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn patch_at(&mut self, at: u32, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        let at = at as usize;
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

use obc_storage::flat::seam::Store;

/// Request bytes, written at the offsets `FLAT_Store_Protocol.md` states.
pub mod client {
    use super::*;

    pub fn frame(opcode: u8, request: u32, body: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; HEADER_LEN + body.len()];
        record[0..4].copy_from_slice(b"OBC4");
        record[4] = 4;
        record[5] = opcode;
        record[8..10].copy_from_slice(&(body.len() as u16).to_le_bytes());
        record[12..16].copy_from_slice(&request.to_le_bytes());
        record[HEADER_LEN..].copy_from_slice(body);
        record
    }

    /// The first page.
    pub fn list(request: u32, kind: Option<u16>) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..2].copy_from_slice(&kind.unwrap_or(0).to_le_bytes());
        frame(0x01, request, &body)
    }

    /// A page resuming after a `(ObjectId, Revision)` pair.
    pub fn list_from(request: u32, kind: Option<u16>, cursor: (u64, u64), sequence: u64) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..2].copy_from_slice(&kind.unwrap_or(0).to_le_bytes());
        body[2..4].copy_from_slice(&1u16.to_le_bytes());
        body[8..16].copy_from_slice(&cursor.0.to_le_bytes());
        body[16..24].copy_from_slice(&cursor.1.to_le_bytes());
        body[24..32].copy_from_slice(&sequence.to_le_bytes());
        frame(0x01, request, &body)
    }

    pub fn status(request: u32, id: u64, revision: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&revision.to_le_bytes());
        frame(0x02, request, &body)
    }

    pub fn get(request: u32, id: u64, revision: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&revision.to_le_bytes());
        frame(0x03, request, &body)
    }

    pub fn put(request: u32, id: u64, expected: u64, bytes: &[u8], kind: u16, name: &str) -> Vec<u8> {
        let mut body = vec![0u8; 84];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        body[16..24].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        body[24..28].copy_from_slice(&crc32(bytes).to_le_bytes());
        body[28..30].copy_from_slice(&kind.to_le_bytes());
        body[32] = name.len() as u8;
        body[36..36 + name.len()].copy_from_slice(name.as_bytes());
        frame(0x04, request, &body)
    }

    pub fn remove(request: u32, id: u64, expected: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&id.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        frame(0x05, request, &body)
    }

    pub fn cancel(request: u32, transfer: u32) -> Vec<u8> {
        frame(0x06, request, &transfer.to_le_bytes())
    }

    pub fn arm(request: u32, package: u64, expected: u64) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&package.to_le_bytes());
        body[8..16].copy_from_slice(&expected.to_le_bytes());
        frame(0x07, request, &body)
    }

    /// The expected identity is the destructive compare-and-swap; the replacement starts the new
    /// store era and must be non-zero and different.
    pub fn format(request: u32, expected: [u8; 16], replacement: [u8; 16]) -> Vec<u8> {
        let mut body = vec![0u8; 32];
        body[0..16].copy_from_slice(&expected);
        body[16..32].copy_from_slice(&replacement);
        frame(0x08, request, &body)
    }

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

    /// One body byte: a `CANCEL` answer is exactly one.
    pub fn byte_at(&self, at: usize) -> u8 {
        self.body[at]
    }

    pub fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.body[at..at + 4].try_into().unwrap())
    }
}
