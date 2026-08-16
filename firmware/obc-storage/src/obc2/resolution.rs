//! The resolution generation (`OBC2_Storage_Format.md` §8).
//!
//! Publishing a manifest removes every draft-part row of its parent, and with them the only place
//! on the card that says which generation each `DraftPartRef` stood for. The resolution generation
//! is what closes that gap: a small store-private table, written once as an ordinary immutable GEN
//! payload before the terminal record, and named by the published head's own resolution field.
//! Garbage collection walks *this*, never the manifest payload.
//!
//! It has no gate. §8: "it is an ordinary immutable GEN payload, written once in one shot and never
//! resumable, so it needs no WORK file, and a cut during the write leaves a body those two checks
//! reject" — the two checks being the count bound and the exact file length. A finalization retried
//! after such a cut rewrites the same reserved generation from offset zero, so no orphan
//! accumulates.

use obc_link::ids::{DraftPartRef, GenerationId};

use super::error::{DecodeError, Reason, Record, Result};
use super::limits::MAX_MANIFEST_CHILDREN;
use super::raw::{bytes16_at, put_bytes, put_u32, put_u64, u32_at, u64_at};

/// The fixed header before the entries.
pub const HEADER_LEN: usize = 8;
/// One entry: a 16-byte reference then a `u64` generation.
pub const ENTRY_LEN: usize = 24;

/// The exact file length of a table with `n` entries.
pub const fn body_len(entries: usize) -> usize {
    HEADER_LEN + entries * ENTRY_LEN
}

/// The largest table: the 32-child maximum, 776 bytes, which is the bounded read a full
/// reachability pass costs per manifest head (§9).
pub const MAX_BODY_LEN: usize = body_len(MAX_MANIFEST_CHILDREN);

/// One `(DraftPartRef, GenerationId)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionEntry {
    /// The opaque reference a manifest names.
    pub part_ref: DraftPartRef,
    /// The generation it denotes.
    pub generation: GenerationId,
}

/// A decoded resolution table, borrowed from the caller's read buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution<'a> {
    entries: &'a [u8],
    count: usize,
}

impl<'a> Resolution<'a> {
    /// Decodes a complete body.
    ///
    /// The two checks §8 names are the count bound — `1..=32`, and equal to the parent's declared
    /// part count, which only the parent's row can prove — and the exact length. Entries must be
    /// ordered by `DraftPartRef` bytes compared lexicographically, and unique.
    pub fn decode(bytes: &'a [u8]) -> Result<Self> {
        const R: Record = Record::Resolution;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() < HEADER_LEN {
            return Err(err(Reason::Length));
        }
        let count = u32_at(bytes, 0) as usize;
        if count == 0 || count > MAX_MANIFEST_CHILDREN {
            return Err(err(Reason::Count));
        }
        if bytes.len() != body_len(count) {
            return Err(err(Reason::Length));
        }
        if u32_at(bytes, 4) != 0 {
            return Err(err(Reason::Reserved));
        }
        let entries = &bytes[HEADER_LEN..];
        let mut previous: Option<[u8; 16]> = None;
        for index in 0..count {
            let key = bytes16_at(entries, index * ENTRY_LEN);
            if let Some(previous) = previous {
                if key < previous {
                    return Err(err(Reason::Order));
                }
                if key == previous {
                    return Err(err(Reason::Duplicate));
                }
            }
            previous = Some(key);
        }
        Ok(Resolution { entries, count })
    }

    /// The entry count.
    pub fn len(&self) -> usize {
        self.count
    }

    /// True when the table is empty, which decoding never produces: §8 bounds the count at `1..=32`.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Entry `index`.
    pub fn get(&self, index: usize) -> Option<ResolutionEntry> {
        if index >= self.count {
            return None;
        }
        let offset = index * ENTRY_LEN;
        Some(ResolutionEntry {
            part_ref: DraftPartRef::new(bytes16_at(self.entries, offset)),
            generation: GenerationId::new(u64_at(self.entries, offset + 16)),
        })
    }

    /// Every entry, in stored order.
    pub fn iter(&self) -> impl Iterator<Item = ResolutionEntry> + '_ {
        (0..self.count).filter_map(move |index| self.get(index))
    }

    /// The generation a reference denotes, by lookup. §8: nothing is decoded out of a reference —
    /// the stored row is the whole authority — so a forged or foreign one simply misses.
    pub fn resolve(&self, part_ref: DraftPartRef) -> Option<GenerationId> {
        self.iter().find(|entry| entry.part_ref == part_ref).map(|entry| entry.generation)
    }
}

/// Encodes a table into `out`, returning the bytes written.
///
/// The caller supplies entries already sorted by reference bytes; encoding refuses anything else
/// rather than sorting, because the order is a property of the bytes the manifest was validated
/// against, not a formatting choice.
pub fn encode(entries: &[ResolutionEntry], out: &mut [u8]) -> Result<usize> {
    const R: Record = Record::Resolution;
    let err = |reason| DecodeError::new(R, reason);
    if entries.is_empty() || entries.len() > MAX_MANIFEST_CHILDREN {
        return Err(err(Reason::Count));
    }
    let len = body_len(entries.len());
    if out.len() < len {
        return Err(err(Reason::Length));
    }
    let mut previous: Option<[u8; 16]> = None;
    for entry in entries {
        let key = entry.part_ref.to_bytes();
        if let Some(previous) = previous {
            if key < previous {
                return Err(err(Reason::Order));
            }
            if key == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous = Some(key);
    }
    out[..len].fill(0);
    put_u32(out, 0, entries.len() as u32);
    for (index, entry) in entries.iter().enumerate() {
        let offset = HEADER_LEN + index * ENTRY_LEN;
        put_bytes(out, offset, entry.part_ref.as_bytes());
        put_u64(out, offset + 16, entry.generation.get());
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(first: u8, generation: u64) -> ResolutionEntry {
        let mut bytes = [0u8; 16];
        bytes[0] = first;
        ResolutionEntry { part_ref: DraftPartRef::new(bytes), generation: GenerationId::new(generation) }
    }

    #[test]
    fn one_entry_is_32_bytes_and_the_maximum_is_776() {
        assert_eq!(body_len(1), 32);
        assert_eq!(MAX_BODY_LEN, 776);
    }

    #[test]
    fn round_trips_and_resolves_by_lookup() {
        let entries = [entry(1, 10), entry(2, 20), entry(3, 30)];
        let mut buffer = [0u8; MAX_BODY_LEN];
        let len = encode(&entries, &mut buffer).unwrap();
        let table = Resolution::decode(&buffer[..len]).unwrap();
        assert_eq!(table.len(), 3);
        assert_eq!(table.resolve(entries[1].part_ref).unwrap().get(), 20);
        assert!(table.resolve(entry(9, 0).part_ref).is_none());
    }

    /// §8's two checks are exactly what a torn one-shot write fails.
    #[test]
    fn a_truncated_body_whose_count_and_length_disagree_is_rejected() {
        let entries = [entry(1, 10), entry(2, 20)];
        let mut buffer = [0u8; MAX_BODY_LEN];
        let len = encode(&entries, &mut buffer).unwrap();
        assert_eq!(Resolution::decode(&buffer[..len - 1]).unwrap_err().reason, Reason::Length);
        assert_eq!(Resolution::decode(&buffer[..HEADER_LEN]).unwrap_err().reason, Reason::Length);
    }

    #[test]
    fn a_count_outside_one_through_thirty_two_is_rejected() {
        let mut buffer = [0u8; MAX_BODY_LEN];
        assert_eq!(Resolution::decode(&buffer[..HEADER_LEN]).unwrap_err().reason, Reason::Count);
        put_u32(&mut buffer, 0, 33);
        assert_eq!(Resolution::decode(&buffer).unwrap_err().reason, Reason::Count);
    }

    #[test]
    fn entries_must_be_ordered_and_unique_by_reference_bytes() {
        let mut buffer = [0u8; MAX_BODY_LEN];
        assert_eq!(encode(&[entry(2, 20), entry(1, 10)], &mut buffer).unwrap_err().reason, Reason::Order);
        assert_eq!(encode(&[entry(1, 20), entry(1, 10)], &mut buffer).unwrap_err().reason, Reason::Duplicate);

        let len = encode(&[entry(1, 10), entry(2, 20)], &mut buffer).unwrap();
        buffer[HEADER_LEN] = 3; // now 3 then 2: out of order in the stored bytes
        assert_eq!(Resolution::decode(&buffer[..len]).unwrap_err().reason, Reason::Order);
    }

    #[test]
    fn the_thirty_two_child_maximum_encodes_at_exactly_776_bytes() {
        let entries: [ResolutionEntry; MAX_MANIFEST_CHILDREN] =
            core::array::from_fn(|index| entry(index as u8, index as u64));
        let mut buffer = [0u8; MAX_BODY_LEN];
        assert_eq!(encode(&entries, &mut buffer).unwrap(), 776);
        assert_eq!(Resolution::decode(&buffer).unwrap().len(), 32);
    }
}
