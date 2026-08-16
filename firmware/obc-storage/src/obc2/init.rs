//! The incomplete-initialization witness, `INIT.REC` (`OBC2_Storage_Format.md` §12).
//!
//! Initialization writes this record before it creates anything else that could outlive a cut, and
//! deletes it once the first checkpoint gate — the StoreId birth point — is durable. Its only job
//! is to say "this unadvertised StoreId owns the preallocation prefix on this card", which is what
//! lets a cut mid-initialization resume instead of restarting with a new identity.
//!
//! It has no sequence and no epoch of its own, so §12 binds its gate to its body by copying the
//! StoreId into the two gate fields that would otherwise carry them: bytes `0..8` become the gate's
//! scope and bytes `8..16` its logical sequence, "solely to bind the two records".

use obc_link::ids::StoreId;

use super::error::{DecodeError, Reason, Record, Result};
use super::gate::{BodyBinding, Gate, MAGIC_INIT};
use super::limits::{SLOT_FILE_LEN, SMALL_BODY_CRC_OFFSET, SMALL_BODY_LEN, SMALL_GATE_OFFSET};
use super::raw::{bytes16_at, crc32_with_hole, put_bytes, put_u16, put_u32, require_zero, u16_at, u32_at, u64_at};

/// Body magic.
pub const MAGIC: [u8; 4] = *b"O2IN";
/// The header length the body declares.
pub const HEADER_LEN: usize = 24;

/// The initialization witness (§12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitRecord {
    /// The 128 CSPRNG bits this initialization attempt owns. It has never escaped `CardStore`.
    pub store: StoreId,
}

impl InitRecord {
    /// The gate scope and sequence §12 derives from the StoreId to bind gate to body.
    fn binding(store: StoreId) -> (u64, u64) {
        let bytes = store.to_bytes();
        (u64_at(&bytes, 0), u64_at(&bytes, 8))
    }

    /// Encodes the 512-byte body with its CRC stamped.
    pub fn encode_body(&self) -> [u8; SMALL_BODY_LEN] {
        let mut out = [0u8; SMALL_BODY_LEN];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        let crc = crc32_with_hole(&out, SMALL_BODY_CRC_OFFSET);
        put_u32(&mut out, SMALL_BODY_CRC_OFFSET, crc);
        out
    }

    /// Encodes the complete 16,384-byte file: body, gate at offset 512, and the zero pad.
    pub fn encode_slot(&self) -> [u8; SLOT_FILE_LEN] {
        let mut out = [0u8; SLOT_FILE_LEN];
        let body = self.encode_body();
        put_bytes(&mut out, 0, &body);
        let (scope, sequence) = Self::binding(self.store);
        let gate = Gate { magic: MAGIC_INIT, slot: 0, scope, sequence, body_crc: u32_at(&body, SMALL_BODY_CRC_OFFSET) };
        put_bytes(&mut out, SMALL_GATE_OFFSET, &gate.encode());
        out
    }

    /// Decodes the 512-byte body.
    pub fn decode_body(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::Init;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != SMALL_BODY_LEN {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != super::gate::FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) as usize != HEADER_LEN {
            return Err(err(Reason::HeaderLength));
        }
        require_zero(R, bytes, 24, SMALL_BODY_CRC_OFFSET - 24)?;
        Ok(InitRecord { store: StoreId::new(bytes16_at(bytes, 8)) })
    }

    /// Validates the complete file: body, gate binding, and a zero pad to the stride.
    pub fn validate_slot(slot_bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::Init;
        if slot_bytes.len() != SLOT_FILE_LEN {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let record = Self::decode_body(&slot_bytes[..SMALL_BODY_LEN])?;
        let gate = Gate::decode(&slot_bytes[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512], MAGIC_INIT, 0)?;
        let (scope, sequence) = Self::binding(record.store);
        gate.bind(&BodyBinding {
            stored_crc: u32_at(slot_bytes, SMALL_BODY_CRC_OFFSET),
            fresh_crc: crc32_with_hole(&slot_bytes[..SMALL_BODY_LEN], SMALL_BODY_CRC_OFFSET),
            scope,
            sequence,
        })?;
        require_zero(R, slot_bytes, 1_024, SLOT_FILE_LEN - 1_024)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_binds_its_gate_to_the_store_id() {
        let record = InitRecord { store: StoreId::new([0x3C; 16]) };
        assert_eq!(InitRecord::validate_slot(&record.encode_slot()).unwrap(), record);
    }

    /// The gate carries the StoreId's own bytes, so a witness whose body names another store does
    /// not validate against it — which is the whole point of the copy.
    #[test]
    fn a_body_from_another_store_under_this_gate_is_invalid() {
        let mut slot = InitRecord { store: StoreId::new([0x3C; 16]) }.encode_slot();
        let other = InitRecord { store: StoreId::new([0x11; 16]) }.encode_body();
        slot[..SMALL_BODY_LEN].copy_from_slice(&other);
        assert!(InitRecord::validate_slot(&slot).is_err());
    }

    #[test]
    fn a_nonzero_reserved_run_or_pad_is_rejected() {
        let mut slot = InitRecord { store: StoreId::new([0x3C; 16]) }.encode_slot();
        slot[100] = 1;
        assert_eq!(InitRecord::validate_slot(&slot).unwrap_err().reason, Reason::Reserved);
        let mut slot = InitRecord { store: StoreId::new([0x3C; 16]) }.encode_slot();
        slot[9_000] = 1;
        assert_eq!(InitRecord::validate_slot(&slot).unwrap_err().reason, Reason::Reserved);
    }
}
