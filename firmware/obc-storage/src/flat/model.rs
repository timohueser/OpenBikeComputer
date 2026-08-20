//! The reference model, host-only: what the card *should* hold, computed without touching one.
//!
//! `FLAT_Store_Format.md` §5.3 is what makes this worth having: "the byte image of the catalog is a
//! function of the store's state, which is what lets the FS3 reference model compare bytes rather
//! than sets". So the oracle is not "the same entries somehow" — it is the exact 512 + `n` × 128
//! bytes, the exact commit sequence, the exact `ObjectId` cursor and the exact free-extent count.
//!
//! The model applies a batch the way §5.5 says a commit does and nothing else: no media, no gates, no
//! ordering. A crash matrix reboots a torn card, mounts it, and requires the result to equal this
//! model either before the batch or after it.

use std::vec::Vec;

use super::catalog::{Entry, Header};
use super::device::BlockDevice;
use super::layout::{body_len, Geometry, BLOCK};
use super::seam::{EntryFlags, EntryMeta, ObjectId, Revision, StoreId};
use super::store::FlatStore;

/// One entry mutation, as the model sees it: the entry to write — extents included, because the model
/// compares bytes — or the key to remove.
#[derive(Debug, Clone)]
pub enum Change {
    Put(Entry),
    Remove((ObjectId, Revision)),
}

/// The store's logical state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub store: StoreId,
    pub sequence: u64,
    /// The greatest sequence any well-formed gate carries, which is what the next commit continues
    /// from. Equal to `sequence` except after a mount that fell back to the older copy.
    pub high_water: u64,
    pub next_object: u64,
    pub entries: Vec<Entry>,
    pub extents: u32,
    /// The card's extent size (§4). §8 gives every card of 64 GiB or less the 1 MiB default, so a
    /// scenario only sets this when it is modelling a bigger one.
    pub geometry: Geometry,
    /// What §7.3 must recover: the ride's flushed length and its length at the newest slot. `None`
    /// when no entry is recording.
    pub ride: Option<(u64, u64)>,
}

/// Everything a mounted card observably is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub sequence: u64,
    /// The mark §5.5 step 2 continues from, which a fallback mount leaves above `sequence`.
    pub high_water: u64,
    pub next_object: u64,
    pub entries: Vec<EntryMeta>,
    pub free_extents: u32,
    /// The serving copy's body bytes, exactly `512 + n × 128` of them.
    pub body: Vec<u8>,
    /// Every readable object's payload, read back through the seam and hashed. The catalog states a
    /// length and a CRC for each; this is whether the bytes on the card agree, which is the one thing
    /// a byte-image comparison of the *catalog* cannot see.
    pub payloads: Vec<(ObjectId, Revision, u32)>,
    /// The recovered ride, as `(flushed, payload length)`.
    pub ride: Option<(u64, u64)>,
}

impl Model {
    /// The card §8 leaves behind: commit sequence `1`, next `ObjectId` `1`, no entries.
    pub fn empty(store: StoreId, extents: u32) -> Self {
        Model {
            store,
            sequence: 1,
            high_water: 1,
            next_object: 1,
            entries: Vec::new(),
            extents,
            geometry: Geometry::DEFAULT,
            ride: None,
        }
    }

    /// §5.5: one batch, applied atomically. The entry array stays sorted by `(ObjectId, Revision)`,
    /// the cursor never rewinds, and the sequence is one past the high-water mark.
    pub fn apply(&mut self, changes: &[Change]) -> &mut Self {
        for change in changes {
            match change {
                Change::Put(entry) => {
                    let key = entry.meta.key();
                    self.entries.retain(|held| held.meta.key() != key);
                    let at = self.entries.partition_point(|held| held.meta.key() < key);
                    self.entries.insert(at, *entry);
                    self.next_object = self.next_object.max(entry.meta.id.0 + 1);
                }
                Change::Remove(key) => self.entries.retain(|held| held.meta.key() != *key),
            }
        }
        self.sequence = self.high_water + 1;
        self.high_water = self.sequence;
        self.ride = if self.entries.iter().any(|entry| entry.meta.flags.has(EntryFlags::RECORDING)) {
            self.ride.or(Some((0, 0)))
        } else {
            None
        };
        self
    }

    /// The catalog body, byte for byte.
    pub fn body(&self) -> Vec<u8> {
        let header = Header {
            store: self.store,
            sequence: self.sequence,
            next_object: self.next_object,
            entry_count: self.entries.len() as u16,
        };
        let mut body = Vec::with_capacity(body_len(self.entries.len() as u16));
        body.extend_from_slice(&header.encode());
        for entry in &self.entries {
            body.extend_from_slice(&entry.encode());
        }
        body
    }

    /// Extents the catalog names — and therefore the ones no allocation may hand out.
    pub fn used_extents(&self) -> Vec<u32> {
        let mut used: Vec<u32> = self
            .entries
            .iter()
            .flat_map(|entry| entry.ranges.iter())
            .flat_map(|(first, count)| first as u32..first as u32 + count as u32)
            .collect();
        used.sort_unstable();
        used.dedup();
        used
    }

    /// Entries whose payload a reader can ask for, and the CRC the catalog claims for each. A reserve
    /// has no bytes the store wrote, and a recording ride's length is zero until the commit that ends
    /// it, so neither is readable.
    pub fn payloads(&self) -> Vec<(ObjectId, Revision, u32)> {
        self.entries
            .iter()
            .filter(|entry| entry.meta.payload_len > 0 && !entry.meta.flags.has(EntryFlags::RESERVED))
            .map(|entry| (entry.meta.id, entry.meta.revision, entry.meta.payload_crc))
            .collect()
    }

    /// What a mount of a card in this state must produce.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            sequence: self.sequence,
            high_water: self.high_water,
            next_object: self.next_object,
            entries: self.entries.iter().map(|entry| entry.meta).collect(),
            free_extents: self.extents - self.used_extents().len() as u32,
            body: self.body(),
            payloads: self.payloads(),
            ride: self.ride,
        }
    }
}

/// What a mounted store observably is, read back through the seam and off the card.
///
/// The body comes from the copy the store says it is serving, so a commit that wrote the wrong copy
/// or left the wrong one selected shows up as a byte difference rather than as a passing test.
pub fn snapshot<D: BlockDevice>(store: &mut FlatStore<D>) -> Option<Snapshot> {
    use super::seam::Store;
    if !store.mode().readable() {
        return None;
    }
    let entries: Vec<EntryMeta> = store.entries().collect();
    // §2's listing has no way to report a read failure, so the harness asks the store whether the
    // iterator it just drained was complete instead of accepting a short list as the truth.
    assert!(store.entries_ok(), "the entry listing was truncated by a media failure");
    let want = body_len(store.entry_count());
    let mut body = Vec::new();
    let mut block = [0u8; BLOCK];
    let base = super::layout::CATALOG[store.serving_copy()];
    for index in 0..want.div_ceil(BLOCK) {
        store.device().read(base + index as u64, &mut block).ok()?;
        body.extend_from_slice(&block);
    }
    body.truncate(want);

    // Every readable payload, read back through the seam and hashed. This is what proves the bytes the
    // catalog describes are on the card.
    let mut payloads = Vec::new();
    for meta in &entries {
        if meta.payload_len == 0 || meta.flags.has(EntryFlags::RESERVED) {
            continue;
        }
        let handle = store.open(meta.id, Some(meta.revision)).expect("a listed entry opens");
        let mut bytes = std::vec![0u8; meta.payload_len as usize];
        let read = store.read(&handle, 0, &mut bytes).expect("a listed entry reads");
        assert_eq!(read as u64, meta.payload_len, "the seam returned a short read inside the payload");
        payloads.push((meta.id, meta.revision, super::raw::crc32(&bytes)));
        store.close(handle);
    }

    Some(Snapshot {
        sequence: store.sequence(),
        high_water: store.high_water(),
        next_object: store.next_object_id().0,
        entries,
        free_extents: store.free_extents(),
        body,
        payloads,
        ride: store.recovered_ride().map(|ride| (ride.flushed, ride.payload_len())),
    })
}
