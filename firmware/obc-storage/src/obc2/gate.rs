//! The common 512-byte gate sector (`OBC2_Storage_Format.md` §4).
//!
//! Every gated record has a body and a physically disjoint gate inside the same 16,384-byte slot.
//! The writer synchronizes the complete body first and writes the gate last, so a gate is the one
//! durable fact that says "this body became a record". Its 512 bytes have exactly one layout
//! everywhere; only the offset it sits at differs per record, and each record's own module fixes
//! that.
//!
//! Validity is all-or-nothing (§4): magic and slot index match the physical location, the version
//! is known, the complement is exact, the body CRC equals both the body's stored CRC and a fresh
//! CRC of the body, scope and sequence equal the body, and the gate CRC validates. There is no
//! partially valid gate and no repair path, so [`Gate::decode`] proves the gate's own structure and
//! [`Gate::bind`] proves the body agreement — a caller that has only one of them has nothing.
//!
//! Invalidation is 512 zero bytes over exactly that sector: an all-zero gate fails the magic and
//! CRC checks, so no distinct sentinel and no read-modify-write is needed.

use super::error::{DecodeError, Record, Result};
use super::limits::GATE_LEN;
use super::raw::{crc32_with_hole, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};

/// Checkpoint gate magic.
pub const MAGIC_CHECKPOINT: [u8; 4] = *b"O2CG";
/// Journal-slot gate magic.
pub const MAGIC_JOURNAL: [u8; 4] = *b"O2JG";
/// `WORK` slot gate magic.
pub const MAGIC_WORK: [u8; 4] = *b"O2WG";
/// `RIDE.ACT` slot gate magic.
pub const MAGIC_RIDE: [u8; 4] = *b"O2RG";
/// Update-handoff gate magic.
pub const MAGIC_HANDOFF: [u8; 4] = *b"O2HG";
/// Initialization-witness gate magic.
pub const MAGIC_INIT: [u8; 4] = *b"O2IG";

/// The format version every OBC2 record carries.
pub const FORMAT_VERSION: u16 = 1;

/// The 512 zero bytes that invalidate a gate.
pub const INVALIDATED: [u8; GATE_LEN] = [0u8; GATE_LEN];

/// One decoded gate sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    /// Gate magic, specific to the containing record.
    pub magic: [u8; 4],
    /// Physical slot index, which must match where the gate was read from.
    pub slot: u16,
    /// Epoch, `GenerationId`, handoff sequence, or initialization binding.
    pub scope: u64,
    /// The logical sequence the body represents.
    pub sequence: u64,
    /// The body's CRC-32, stored twice — once plain and once complemented.
    pub body_crc: u32,
}

/// What a body must agree with for its gate to be valid (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyBinding {
    /// The CRC the body stores in its own CRC field.
    pub stored_crc: u32,
    /// A fresh CRC computed over the body bytes.
    pub fresh_crc: u32,
    /// The scope the body states.
    pub scope: u64,
    /// The logical sequence the body states.
    pub sequence: u64,
}

impl Gate {
    /// Encodes the exact 512 bytes.
    pub fn encode(&self) -> [u8; GATE_LEN] {
        let mut out = [0u8; GATE_LEN];
        put_bytes(&mut out, 0, &self.magic);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_u16(&mut out, 6, self.slot);
        put_u64(&mut out, 8, self.scope);
        put_u64(&mut out, 16, self.sequence);
        put_u32(&mut out, 24, self.body_crc);
        put_u32(&mut out, 28, !self.body_crc);
        let crc = crc32_with_hole(&out, 32);
        put_u32(&mut out, 32, crc);
        out
    }

    /// Decodes and structurally validates a gate read from physical slot `slot` of a record whose
    /// gate magic is `magic`.
    ///
    /// This half proves everything the gate can prove about itself. It deliberately cannot prove
    /// the body agreement — [`Gate::bind`] does that — because the body is a different read.
    pub fn decode(bytes: &[u8], magic: [u8; 4], slot: u16) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Gate, reason);
        if bytes.len() != GATE_LEN {
            return Err(err(super::error::Reason::Length));
        }
        if bytes[0..4] != magic {
            return Err(err(super::error::Reason::Magic));
        }
        if u16_at(bytes, 4) != FORMAT_VERSION {
            return Err(err(super::error::Reason::Version));
        }
        if u16_at(bytes, 6) != slot {
            return Err(err(super::error::Reason::SlotIndex));
        }
        let body_crc = u32_at(bytes, 24);
        if u32_at(bytes, 28) != !body_crc {
            return Err(err(super::error::Reason::Complement));
        }
        if u32_at(bytes, 32) != crc32_with_hole(bytes, 32) {
            return Err(err(super::error::Reason::GateCrc));
        }
        if !is_zero(bytes, 36, GATE_LEN - 36) {
            return Err(err(super::error::Reason::Reserved));
        }
        Ok(Gate { magic, slot, scope: u64_at(bytes, 8), sequence: u64_at(bytes, 16), body_crc })
    }

    /// Proves the body under this gate is the body the gate names.
    pub fn bind(&self, body: &BodyBinding) -> Result<()> {
        let err = |reason| DecodeError::new(Record::Gate, reason);
        if body.stored_crc != body.fresh_crc || self.body_crc != body.fresh_crc {
            return Err(err(super::error::Reason::BodyCrc));
        }
        if self.scope != body.scope {
            return Err(err(super::error::Reason::Scope));
        }
        if self.sequence != body.sequence {
            return Err(err(super::error::Reason::Sequence));
        }
        Ok(())
    }

    /// True when these 512 bytes are an invalidated gate: exactly the zero sector §4 writes.
    ///
    /// A caller never needs this to decide validity — [`Gate::decode`] already refuses a zero
    /// sector — but recovery diagnostics distinguish "deliberately invalidated" from "torn".
    pub fn is_invalidated(bytes: &[u8]) -> bool {
        bytes.len() == GATE_LEN && bytes.iter().all(|&byte| byte == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::super::error::Reason;
    use super::*;

    fn sample() -> Gate {
        Gate { magic: MAGIC_JOURNAL, slot: 3, scope: 7, sequence: 42, body_crc: 0xDEAD_BEEF }
    }

    #[test]
    fn round_trips_and_binds() {
        let gate = sample();
        let bytes = gate.encode();
        let decoded = Gate::decode(&bytes, MAGIC_JOURNAL, 3).unwrap();
        assert_eq!(decoded, gate);
        decoded.bind(&BodyBinding { stored_crc: 0xDEAD_BEEF, fresh_crc: 0xDEAD_BEEF, scope: 7, sequence: 42 }).unwrap();
    }

    #[test]
    fn an_invalidated_gate_is_never_valid() {
        assert!(Gate::is_invalidated(&INVALIDATED));
        assert_eq!(Gate::decode(&INVALIDATED, MAGIC_JOURNAL, 0).unwrap_err().reason, Reason::Magic);
    }

    #[test]
    fn a_gate_read_from_the_wrong_slot_or_record_is_invalid() {
        let bytes = sample().encode();
        assert_eq!(Gate::decode(&bytes, MAGIC_JOURNAL, 2).unwrap_err().reason, Reason::SlotIndex);
        assert_eq!(Gate::decode(&bytes, MAGIC_WORK, 3).unwrap_err().reason, Reason::Magic);
    }

    #[test]
    fn every_single_byte_flip_is_rejected() {
        let bytes = sample().encode();
        for index in 0..GATE_LEN {
            let mut torn = bytes;
            torn[index] ^= 0xFF;
            assert!(Gate::decode(&torn, MAGIC_JOURNAL, 3).is_err(), "byte {index} flip accepted");
        }
    }

    #[test]
    fn body_disagreement_is_rejected_field_by_field() {
        let gate = Gate::decode(&sample().encode(), MAGIC_JOURNAL, 3).unwrap();
        let good = BodyBinding { stored_crc: 0xDEAD_BEEF, fresh_crc: 0xDEAD_BEEF, scope: 7, sequence: 42 };
        assert_eq!(gate.bind(&BodyBinding { fresh_crc: 0, ..good }).unwrap_err().reason, Reason::BodyCrc);
        assert_eq!(gate.bind(&BodyBinding { stored_crc: 0, ..good }).unwrap_err().reason, Reason::BodyCrc);
        assert_eq!(gate.bind(&BodyBinding { scope: 8, ..good }).unwrap_err().reason, Reason::Scope);
        assert_eq!(gate.bind(&BodyBinding { sequence: 43, ..good }).unwrap_err().reason, Reason::Sequence);
    }

    #[test]
    fn version_and_complement_are_checked() {
        let mut bytes = sample().encode();
        bytes[4] = 2;
        let crc = crc32_with_hole(&bytes, 32);
        bytes[32..36].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Gate::decode(&bytes, MAGIC_JOURNAL, 3).unwrap_err().reason, Reason::Version);

        let mut bytes = sample().encode();
        bytes[28] ^= 0x01;
        let crc = crc32_with_hole(&bytes, 32);
        bytes[32..36].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Gate::decode(&bytes, MAGIC_JOURNAL, 3).unwrap_err().reason, Reason::Complement);
    }

    #[test]
    fn a_short_or_long_sector_is_rejected() {
        let bytes = sample().encode();
        assert_eq!(Gate::decode(&bytes[..511], MAGIC_JOURNAL, 3).unwrap_err().reason, Reason::Length);
    }
}
