//! The spec-derived storage fixture producer for `specs/vectors/device-object-v2/storage/`.
//!
//! `Device_Object_Vectors_v2.md` §1 fixes the one rule that makes a golden vector worth having: "A
//! production decoder must not generate its own expected bytes. The Rust fixture producer builds
//! bytes directly from the byte tables without calling the production encoder." So every byte below
//! is laid down by hand at the offset `OBC2_Storage_Format.md` gives it, through the little [`raw`]
//! helpers — never through [`super::journal`], [`super::checkpoint`] or any `encode` method. The
//! tests then close the loop in both directions: the checked-in files must equal what this producer
//! emits, and the production codec must accept every positive case, re-encode it byte for byte, and
//! reject every negative case with the stated reason.
//!
//! ## The `runs` encoding
//!
//! An OBC2 record is mostly zeros — a journal slot is 1,536 meaningful bytes inside a 16,384-byte
//! stride, and a checkpoint 65,024 bytes of which all but a few hundred are a zeroed region. Hex of
//! the whole thing would be unreviewable, so each case states its `length` and the non-zero `runs`
//! at their offsets, plus the `sha256` of the fully materialized bytes. Reconstruction is exact and
//! mechanical: allocate `length` zeros, splice each run in, check the digest.
//!
//! Regenerate after a deliberate spec change with:
//!
//! ```text
//! cargo test -p obc-storage regenerate_storage_vectors -- --ignored
//! ```
//!
//! and then regenerate the suite manifest, which indexes these files:
//!
//! ```text
//! cargo test -p obc-link regenerate -- --ignored
//! ```
//!
//! ## Scope
//!
//! This is the slice 1 + 2 inventory: the record layouts of §4 through §12, their rejections, and
//! the crash-cut transcripts of the commit paths those slices implement. §6's remaining storage
//! items — filesystem-shape images, import staging, lease and GC behaviour, the compaction
//! materialization sources — arrive with the slices that implement them.

use std::format;
use std::path::PathBuf;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use super::limits::{
    CHECKPOINT_BODY_CRC_OFFSET, CHECKPOINT_BODY_LEN, CHECKPOINT_FILE_LEN, CHECKPOINT_GATE_OFFSET, GATE_LEN,
    JOURNAL_BODY_CRC_OFFSET, JOURNAL_GATE_OFFSET, MUTATION_LEN, SLOT_FILE_LEN, SLOT_STRIDE, SMALL_BODY_CRC_OFFSET,
    SMALL_GATE_OFFSET,
};

/// Hand-built little-endian byte assembly. Deliberately tiny and deliberately not the codec.
pub mod raw {
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    /// A zero-filled buffer of exactly `len` bytes, to be filled at stated offsets.
    pub fn zeros(len: usize) -> Vec<u8> {
        vec![0u8; len]
    }

    /// Writes a `u16` at `offset`.
    pub fn u16_at(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u32` at `offset`.
    pub fn u32_at(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u64` at `offset`.
    pub fn u64_at(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes an `i32` at `offset`.
    pub fn i32_at(buffer: &mut [u8], offset: usize, value: i32) {
        u32_at(buffer, offset, value as u32);
    }

    /// Writes an `i64` at `offset`.
    pub fn i64_at(buffer: &mut [u8], offset: usize, value: i64) {
        u64_at(buffer, offset, value as u64);
    }

    /// Writes raw bytes at `offset`.
    pub fn bytes_at(buffer: &mut [u8], offset: usize, value: &[u8]) {
        buffer[offset..offset + value.len()].copy_from_slice(value);
    }

    /// Lower-case hex of a byte slice.
    pub fn hex(bytes: &[u8]) -> String {
        use core::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The §1 CRC-32/IEEE, computed the long way so a fixture never inherits a bug from the shared
    /// implementation it exists to pin.
    pub fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    /// The §1 CRC of a record whose own four-byte CRC field lies inside the checksummed range: "A
    /// CRC field is treated as zero while its containing record is checksummed."
    pub fn crc32_with_hole(bytes: &[u8], hole: usize) -> u32 {
        let mut copy = bytes.to_vec();
        copy[hole..hole + 4].copy_from_slice(&[0, 0, 0, 0]);
        crc32(&copy)
    }

    /// SHA-256, for the manifest and for a case's materialized bytes.
    pub fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex(&hasher.finalize())
    }
}

use raw::{bytes_at, crc32_with_hole, hex, i32_at, i64_at, sha256, u16_at, u32_at, u64_at, zeros};

/// The `specs/vectors/device-object-v2/storage` directory at the repo root.
pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/device-object-v2/storage")
}

// ---------------------------------------------------------------------------------------------
// Fixture identities. The same ones `super::samples` uses, so a vector and a unit test describe
// the same record.
// ---------------------------------------------------------------------------------------------

/// The suite's StoreId.
pub const STORE: [u8; 16] =
    [0x3c, 0x92, 0x00, 0x00, 0x99, 0x16, 0x4e, 0xba, 0xab, 0xc2, 0x34, 0x2f, 0xe0, 0x8f, 0x6b, 0x10];
const OP_A: [u8; 16] = [0xa1; 16];
const OP_PARENT: [u8; 16] = [0xc3; 16];
const OP_CHILD: [u8; 16] = [0xd4; 16];
const OP_INSTALL: [u8; 16] = [0xe5; 16];
const OP_ABORT: [u8; 16] = [0xf6; 16];
const OP_RIDE: [u8; 16] = [0xc1; 16];
const PART_REF: [u8; 16] = [0x5a; 16];
const INTENT: [u8; 32] = [0x11; 32];
const PRINCIPAL: [u8; 32] = [0x22; 32];

// ---------------------------------------------------------------------------------------------
// Hand-built records, straight from the offset tables
// ---------------------------------------------------------------------------------------------

/// §4's 512-byte gate.
pub fn gate(magic: &[u8; 4], slot: u16, scope: u64, sequence: u64, body_crc: u32) -> Vec<u8> {
    let mut out = zeros(GATE_LEN);
    bytes_at(&mut out, 0, magic);
    u16_at(&mut out, 4, 1);
    u16_at(&mut out, 6, slot);
    u64_at(&mut out, 8, scope);
    u64_at(&mut out, 16, sequence);
    u32_at(&mut out, 24, body_crc);
    u32_at(&mut out, 28, !body_crc);
    let crc = crc32_with_hole(&out, 32);
    u32_at(&mut out, 32, crc);
    out
}

/// §5.3's 24-byte repository state.
fn repository_entry(kind: u16, revision: u64, next_id: u64) -> Vec<u8> {
    let mut out = zeros(24);
    u16_at(&mut out, 0, kind);
    u64_at(&mut out, 8, revision);
    u64_at(&mut out, 16, next_id);
    out
}

/// §5.3's 160-byte catalog head.
fn head_entry(kind: u16, id: u64, resolution: Option<u64>) -> Vec<u8> {
    let mut out = zeros(160);
    out[0] = 1;
    out[1] = u8::from(resolution.is_some());
    u16_at(&mut out, 2, kind);
    u64_at(&mut out, 4, id);
    u64_at(&mut out, 12, 9);
    u64_at(&mut out, 20, 42);
    u64_at(&mut out, 28, 3_000);
    u32_at(&mut out, 36, 0x1234_5678);
    u16_at(&mut out, 40, 8);
    bytes_at(&mut out, 48, &[1, 2, 3, 4, 5, 6, 7, 8]);
    if let Some(generation) = resolution {
        u64_at(&mut out, 144, generation);
    }
    out
}

/// §5.3's 128-byte active operation.
fn active_entry(operation: &[u8; 16], opcode: u16, phase: u8, flags: u8) -> Vec<u8> {
    let mut out = zeros(128);
    bytes_at(&mut out, 0, operation);
    bytes_at(&mut out, 16, &INTENT);
    bytes_at(&mut out, 48, &PRINCIPAL);
    u16_at(&mut out, 80, opcode);
    u16_at(&mut out, 82, 1);
    out[84] = phase;
    out[85] = flags;
    u64_at(&mut out, 88, 7);
    u64_at(&mut out, 104, 42);
    u64_at(&mut out, 112, 3);
    out
}

/// §5.3's 128-byte draft parent.
fn parent_entry(state: u8) -> Vec<u8> {
    let mut out = zeros(128);
    bytes_at(&mut out, 0, &OP_PARENT);
    bytes_at(&mut out, 16, &[0x44; 32]);
    u64_at(&mut out, 48, 90);
    u16_at(&mut out, 56, 6);
    u16_at(&mut out, 58, 2);
    out[60] = state;
    u64_at(&mut out, 80, 776);
    u32_at(&mut out, 88, 0x0f0f_0f0f);
    u64_at(&mut out, 96, 1);
    out
}

/// §5.3's 96-byte draft part.
fn part_entry(key: u64, state: u8, with_ref: bool) -> Vec<u8> {
    let mut out = zeros(96);
    bytes_at(&mut out, 0, &OP_PARENT);
    bytes_at(&mut out, 16, &OP_CHILD);
    if with_ref {
        bytes_at(&mut out, 32, &PART_REF);
    }
    u16_at(&mut out, 48, 1);
    u64_at(&mut out, 52, key);
    u64_at(&mut out, 60, 91);
    u64_at(&mut out, 68, 1_024);
    u32_at(&mut out, 76, 0x1111_2222);
    out[80] = state;
    out
}

/// §5.3's 64-byte retained-previous entry.
fn retained_entry(generation: u64, reasons: u8, lease_count: u16) -> Vec<u8> {
    let mut out = zeros(64);
    out[0] = 1;
    out[1] = reasons;
    u16_at(&mut out, 2, lease_count);
    u16_at(&mut out, 4, 1);
    u64_at(&mut out, 8, 7);
    u64_at(&mut out, 16, generation);
    u64_at(&mut out, 24, 3_000);
    u32_at(&mut out, 32, 0xaabb_ccdd);
    u64_at(&mut out, 48, 8);
    out
}

/// §5.3's 208-byte terminal result.
fn result_entry(commit_sequence: u64, operation: &[u8; 16]) -> Vec<u8> {
    typed_result_entry(commit_sequence, operation, 1, 1, &[0x5a; 64])
}

/// The same entry at any `(terminal state, result type)` with an explicit body, so the length
/// column of §5.3's table can be exercised value by value.
fn typed_result_entry(
    commit_sequence: u64,
    operation: &[u8; 16],
    terminal_state: u8,
    result_type: u8,
    body: &[u8],
) -> Vec<u8> {
    let mut out = zeros(208);
    u64_at(&mut out, 0, commit_sequence);
    bytes_at(&mut out, 8, operation);
    bytes_at(&mut out, 24, &INTENT);
    bytes_at(&mut out, 56, &PRINCIPAL);
    out[88] = terminal_state;
    out[89] = result_type;
    u16_at(&mut out, 90, body.len() as u16);
    bytes_at(&mut out, 104, body);
    out
}

/// §5.3's `DomainResult`: "OperationId `[16]`, StoreId `[16]`, ObjectKind/domain `u16`, outcome
/// `u16`, domain-state revision `u64`, and reserved zero `u32`, exactly 48 bytes."
fn domain_result_body(operation: &[u8; 16], domain: u16, outcome: u16, revision: u64) -> Vec<u8> {
    let mut out = zeros(48);
    bytes_at(&mut out, 0, operation);
    bytes_at(&mut out, 16, &STORE);
    u16_at(&mut out, 32, domain);
    u16_at(&mut out, 34, outcome);
    u64_at(&mut out, 36, revision);
    out
}

/// §5.3's 80-byte weather state.
fn weather_entry() -> Vec<u8> {
    let mut out = zeros(80);
    out[0] = 1;
    out[1] = 2;
    u16_at(&mut out, 2, 1);
    u64_at(&mut out, 4, 5);
    u64_at(&mut out, 12, 3);
    u64_at(&mut out, 28, 11);
    i32_at(&mut out, 36, 480_000_000);
    i32_at(&mut out, 40, -1_200_000_000);
    u32_at(&mut out, 44, 40_000);
    i64_at(&mut out, 52, 1_700_000_000);
    i64_at(&mut out, 60, 1_700_086_400);
    u64_at(&mut out, 68, 5);
    out
}

/// §5.3's 128-byte active-ride state.
fn ride_entry() -> Vec<u8> {
    let mut out = zeros(128);
    out[0] = 1;
    out[1] = 1;
    out[2] = 1;
    u64_at(&mut out, 8, 12);
    bytes_at(&mut out, 16, &OP_RIDE);
    bytes_at(&mut out, 32, &[0x33; 32]);
    u64_at(&mut out, 64, 77);
    i64_at(&mut out, 72, 1_700_000_000);
    u64_at(&mut out, 80, 4);
    u64_at(&mut out, 88, 2);
    out
}

/// §10's 240-byte HandoffRef at a phase with no observed outcome yet.
fn handoff_ref(sequence: u64, phase: u8) -> Vec<u8> {
    handoff_ref_full(sequence, phase, 0, 0, 0)
}

/// The same reference with §10's observation fields filled: the OBCU outcome at byte 9, the
/// terminal-result commit sequence at 184, and the observed outcome generation at 192.
fn handoff_ref_full(sequence: u64, phase: u8, outcome: u8, terminal_commit: u64, outcome_generation: u32) -> Vec<u8> {
    let mut out = handoff_ref_base(sequence, phase);
    out[9] = outcome;
    u64_at(&mut out, 184, terminal_commit);
    u32_at(&mut out, 192, outcome_generation);
    out
}

fn handoff_ref_base(sequence: u64, phase: u8) -> Vec<u8> {
    let mut out = zeros(240);
    u64_at(&mut out, 0, sequence);
    out[8] = phase;
    bytes_at(&mut out, 16, &OP_INSTALL);
    bytes_at(&mut out, 32, &[0x55; 32]);
    u64_at(&mut out, 64, 31);
    u64_at(&mut out, 72, 262_144);
    u32_at(&mut out, 80, 0x9999_8888);
    u32_at(&mut out, 84, 7);
    bytes_at(&mut out, 88, &[0x66; 32]);
    bytes_at(&mut out, 120, &[0x77; 64]);
    u64_at(&mut out, 224, 3);
    u64_at(&mut out, 232, 5);
    out
}

/// A complete checkpoint file: §5.1's regions, §5.2's header, the body CRC and the `O2CG` gate.
#[derive(Debug, Clone, Default)]
struct Checkpoint {
    epoch: u64,
    through_sequence: u64,
    next_generation: u64,
    terminal_counter: u64,
    repositories: Vec<Vec<u8>>,
    heads: Vec<Vec<u8>>,
    actives: Vec<Vec<u8>>,
    parent: Option<Vec<u8>>,
    parts: Vec<Vec<u8>>,
    retained: Vec<Vec<u8>>,
    result_start: u8,
    results: Vec<Vec<u8>>,
    handoff: Option<Vec<u8>>,
    weather: Option<Vec<u8>>,
    ride: Option<Vec<u8>>,
}

impl Checkpoint {
    fn file(&self, slot: u16) -> Vec<u8> {
        let mut body = zeros(CHECKPOINT_BODY_LEN);
        bytes_at(&mut body, 0, b"O2CK");
        u16_at(&mut body, 4, 1);
        u16_at(&mut body, 6, 128);
        bytes_at(&mut body, 8, &STORE);
        u64_at(&mut body, 24, self.epoch);
        u64_at(&mut body, 32, self.through_sequence);
        u64_at(&mut body, 40, self.next_generation);
        u16_at(&mut body, 48, self.repositories.len() as u16);
        u16_at(&mut body, 50, self.heads.len() as u16);
        body[52] = self.actives.len() as u8;
        body[53] = u8::from(self.parent.is_some());
        body[54] = self.parts.len() as u8;
        body[55] = self.retained.len() as u8;
        body[56] = self.result_start;
        body[57] = self.results.len() as u8;
        body[58] = u8::from(self.handoff.is_some());
        u64_at(&mut body, 60, self.terminal_counter);
        u32_at(&mut body, 68, CHECKPOINT_BODY_LEN as u32);
        body[104] = u8::from(self.weather.is_some());
        body[105] = u8::from(self.ride.is_some());

        for (index, row) in self.repositories.iter().enumerate() {
            bytes_at(&mut body, 128 + index * 24, row);
        }
        for (index, row) in self.heads.iter().enumerate() {
            bytes_at(&mut body, 512 + index * 160, row);
        }
        for (index, row) in self.actives.iter().enumerate() {
            bytes_at(&mut body, 41_472 + index * 128, row);
        }
        if let Some(parent) = &self.parent {
            bytes_at(&mut body, 42_624, parent);
        }
        for (index, row) in self.parts.iter().enumerate() {
            bytes_at(&mut body, 42_752 + index * 96, row);
        }
        for (index, row) in self.retained.iter().enumerate() {
            bytes_at(&mut body, 45_824 + index * 64, row);
        }
        for (step, row) in self.results.iter().enumerate() {
            let index = (self.result_start as usize + step) % 64;
            bytes_at(&mut body, 46_336 + index * 208, row);
        }
        if let Some(handoff) = &self.handoff {
            bytes_at(&mut body, 59_648, handoff);
        }
        if let Some(weather) = &self.weather {
            bytes_at(&mut body, 59_888, weather);
        }
        if let Some(ride) = &self.ride {
            bytes_at(&mut body, 59_968, ride);
        }
        let crc = crc32_with_hole(&body, CHECKPOINT_BODY_CRC_OFFSET);
        u32_at(&mut body, CHECKPOINT_BODY_CRC_OFFSET, crc);

        let mut file = zeros(CHECKPOINT_FILE_LEN);
        bytes_at(&mut file, 0, &body);
        bytes_at(&mut file, CHECKPOINT_GATE_OFFSET, &gate(b"O2CG", slot, self.epoch, self.through_sequence, crc));
        file
    }
}

/// Rebuilds a mutated checkpoint file's body CRC and its gate, so the only thing that can refuse it
/// is the rule under test.
fn reseal_checkpoint(file: &mut [u8], slot: u16) {
    let crc = crc32_with_hole(&file[..CHECKPOINT_BODY_LEN], CHECKPOINT_BODY_CRC_OFFSET);
    u32_at(file, CHECKPOINT_BODY_CRC_OFFSET, crc);
    let epoch = u64::from_le_bytes(file[24..32].try_into().expect("eight bytes"));
    let through = u64::from_le_bytes(file[32..40].try_into().expect("eight bytes"));
    let gate = gate(b"O2CG", slot, epoch, through, crc);
    bytes_at(file, CHECKPOINT_GATE_OFFSET, &gate);
}

/// A journal record's mutation, built from §6.1's presence bits and entry offsets.
#[derive(Debug, Clone, Default)]
struct Mutation {
    presence: u32,
    repository_kind: u16,
    repository_revision: u64,
    next_logical_id: u64,
    generation_cursor: u64,
    active: Option<Vec<u8>>,
    head: Option<Vec<u8>>,
    parent: Option<Vec<u8>>,
    part: Option<Vec<u8>>,
    retained: Option<Vec<u8>>,
    result: Option<Vec<u8>>,
    handoff: Option<Vec<u8>>,
    weather: Option<Vec<u8>>,
    ride: Option<Vec<u8>>,
}

impl Mutation {
    fn bytes(&self, kind: u16) -> Vec<u8> {
        let mut out = zeros(MUTATION_LEN);
        u16_at(&mut out, 0, 1);
        u32_at(&mut out, 4, self.presence);
        u16_at(&mut out, 8, self.repository_kind);
        u16_at(&mut out, 10, kind);
        u64_at(&mut out, 12, self.repository_revision);
        u64_at(&mut out, 20, self.next_logical_id);
        u64_at(&mut out, 32, self.generation_cursor);
        for (offset, entry) in [
            (40usize, &self.active),
            (168, &self.head),
            (328, &self.parent),
            (456, &self.part),
            (552, &self.retained),
            (616, &self.result),
            (824, &self.handoff),
            (1_064, &self.weather),
            (1_144, &self.ride),
        ] {
            if let Some(bytes) = entry {
                bytes_at(&mut out, offset, bytes);
            }
        }
        out
    }
}

/// A complete journal slot: §6.1's body, its CRC, the `O2JG` gate, and the pad to the stride.
fn journal_slot(
    epoch: u64,
    sequence: u64,
    slot: u16,
    kind: u16,
    identity: Option<&[u8; 16]>,
    mutation: &Mutation,
) -> Vec<u8> {
    let mut body = zeros(1_536);
    bytes_at(&mut body, 0, b"O2JR");
    u16_at(&mut body, 4, 1);
    u16_at(&mut body, 6, 96);
    bytes_at(&mut body, 8, &STORE);
    u64_at(&mut body, 24, epoch);
    u64_at(&mut body, 32, sequence);
    u16_at(&mut body, 40, slot);
    u16_at(&mut body, 42, kind);
    if let Some(operation) = identity {
        bytes_at(&mut body, 44, operation);
        bytes_at(&mut body, 60, &INTENT);
    }
    u16_at(&mut body, 92, MUTATION_LEN as u16);
    bytes_at(&mut body, 96, &mutation.bytes(kind));
    let crc = crc32_with_hole(&body, JOURNAL_BODY_CRC_OFFSET);
    u32_at(&mut body, JOURNAL_BODY_CRC_OFFSET, crc);

    let mut stride = zeros(SLOT_STRIDE);
    bytes_at(&mut stride, 0, &body);
    bytes_at(&mut stride, JOURNAL_GATE_OFFSET, &gate(b"O2JG", slot, epoch, sequence, crc));
    stride
}

/// A 512-byte body inside a full stride, with its gate — the shape WORK, RIDE, ARM and INIT share.
fn small_slot(body: Vec<u8>, magic: &[u8; 4], slot: u16, scope: u64, sequence: u64) -> Vec<u8> {
    let mut body = body;
    let crc = crc32_with_hole(&body, SMALL_BODY_CRC_OFFSET);
    u32_at(&mut body, SMALL_BODY_CRC_OFFSET, crc);
    let mut stride = zeros(SLOT_STRIDE);
    bytes_at(&mut stride, 0, &body);
    bytes_at(&mut stride, SMALL_GATE_OFFSET, &gate(magic, slot, scope, sequence, crc));
    stride
}

/// §7's WORK body.
fn work_body(sequence: u32, offset: u64, state: u8, observed: u32) -> Vec<u8> {
    let mut out = zeros(512);
    bytes_at(&mut out, 0, b"O2WK");
    u16_at(&mut out, 4, 1);
    u16_at(&mut out, 6, 176);
    bytes_at(&mut out, 8, &STORE);
    bytes_at(&mut out, 24, &OP_A);
    bytes_at(&mut out, 40, &INTENT);
    u64_at(&mut out, 104, 42);
    u64_at(&mut out, 112, 65_536);
    u32_at(&mut out, 120, 0x1234_5678);
    out[124] = state;
    u64_at(&mut out, 128, offset);
    u32_at(&mut out, 136, 0x9abc_def0);
    u32_at(&mut out, 140, sequence);
    u64_at(&mut out, 144, 3);
    u16_at(&mut out, 152, 1);
    out[154] = 1;
    u32_at(&mut out, 164, observed);
    out
}

/// §7.1's `RIDE.ACT` body.
fn ride_body(sequence: u32, offset: u64, state: u8, observed: u32) -> Vec<u8> {
    let mut out = zeros(512);
    bytes_at(&mut out, 0, b"O2RA");
    u16_at(&mut out, 4, 1);
    u16_at(&mut out, 6, 140);
    bytes_at(&mut out, 8, &STORE);
    u64_at(&mut out, 24, 12);
    bytes_at(&mut out, 32, &OP_RIDE);
    u64_at(&mut out, 48, 77);
    out[56] = state;
    i64_at(&mut out, 64, 1_700_000_000);
    u64_at(&mut out, 88, offset);
    u32_at(&mut out, 96, 0x2222_3333);
    u32_at(&mut out, 100, sequence);
    u64_at(&mut out, 104, offset / 16);
    u64_at(&mut out, 112, 60_000);
    u32_at(&mut out, 136, observed);
    out
}

/// §10's ARM body.
fn arm_body(sequence: u64, phase: u8) -> Vec<u8> {
    let mut out = zeros(512);
    bytes_at(&mut out, 0, b"O2UH");
    u16_at(&mut out, 4, 1);
    u16_at(&mut out, 6, 64);
    bytes_at(&mut out, 8, &STORE);
    u64_at(&mut out, 24, sequence);
    u16_at(&mut out, 32, 240);
    bytes_at(&mut out, 64, &handoff_ref(sequence, phase));
    out
}

/// §12's `INIT.REC` body.
fn init_body() -> Vec<u8> {
    let mut out = zeros(512);
    bytes_at(&mut out, 0, b"O2IN");
    u16_at(&mut out, 4, 1);
    u16_at(&mut out, 6, 24);
    bytes_at(&mut out, 8, &STORE);
    out
}

/// §8's resolution generation.
fn resolution_body(entries: &[([u8; 16], u64)]) -> Vec<u8> {
    let mut out = zeros(8 + entries.len() * 24);
    u32_at(&mut out, 0, entries.len() as u32);
    for (index, (part_ref, generation)) in entries.iter().enumerate() {
        bytes_at(&mut out, 8 + index * 24, part_ref);
        u64_at(&mut out, 8 + index * 24 + 16, *generation);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cases and files
// ---------------------------------------------------------------------------------------------

/// Which production decoder a case is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A 512-byte gate sector, read as a journal gate at slot 0.
    Gate,
    /// A complete 65,536-byte checkpoint file at slot 0.
    Checkpoint,
    /// A complete 16,384-byte journal slot.
    JournalSlot,
    /// A complete 16,384-byte WORK slot.
    WorkSlot,
    /// A complete 16,384-byte `RIDE.ACT` slot.
    RideSlot,
    /// A complete 16,384-byte ARM file.
    ArmFile,
    /// A complete 16,384-byte `INIT.REC`.
    InitRecord,
    /// A resolution generation body.
    Resolution,
}

impl Subject {
    fn name(self) -> &'static str {
        match self {
            Subject::Gate => "gate",
            Subject::Checkpoint => "checkpoint",
            Subject::JournalSlot => "journalSlot",
            Subject::WorkSlot => "workSlot",
            Subject::RideSlot => "rideSlot",
            Subject::ArmFile => "armFile",
            Subject::InitRecord => "initRecord",
            Subject::Resolution => "resolution",
        }
    }
}

/// One vector case.
#[derive(Debug, Clone)]
pub struct Case {
    /// Stable name.
    pub name: String,
    /// What it proves.
    pub note: String,
    /// Which decoder reads it.
    pub subject: Subject,
    /// The physical slot the bytes are read at.
    pub slot: u16,
    /// The complete bytes.
    pub bytes: Vec<u8>,
    /// `None` for a positive case, or the [`super::error::Reason`] name a rejection must carry.
    pub reject: Option<&'static str>,
}

impl Case {
    fn accept(name: &str, note: &str, subject: Subject, slot: u16, bytes: Vec<u8>) -> Self {
        Case { name: name.to_string(), note: note.to_string(), subject, slot, bytes, reject: None }
    }

    fn reject(name: &str, note: &str, subject: Subject, slot: u16, bytes: Vec<u8>, reason: &'static str) -> Self {
        Case { name: name.to_string(), note: note.to_string(), subject, slot, bytes, reject: Some(reason) }
    }

    fn to_json(&self) -> String {
        let runs: Vec<String> = runs(&self.bytes)
            .into_iter()
            .map(|(offset, bytes)| format!("      {{ \"offset\": {offset}, \"hex\": \"{}\" }}", hex(&bytes)))
            .collect();
        let reject = match self.reject {
            Some(reason) => format!("\"{reason}\""),
            None => "null".to_string(),
        };
        format!(
            "  {{\n    \"name\": \"{}\",\n    \"note\": \"{}\",\n    \"subject\": \"{}\",\n    \"slot\": {},\n    \"length\": {},\n    \"sha256\": \"{}\",\n    \"reject\": {},\n    \"runs\": [\n{}\n    ]\n  }}",
            self.name,
            self.note,
            self.subject.name(),
            self.slot,
            self.bytes.len(),
            sha256(&self.bytes),
            reject,
            runs.join(",\n"),
        )
    }
}

/// The non-zero runs of a record, which is how a vector states bytes that are mostly zeros.
fn runs(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    /// A run closes only after this many consecutive zeros, so a field with an internal zero byte
    /// — every `u64` in these records has several — stays one reviewable run.
    const GAP: usize = 16;
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0 {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index + 1;
        let mut cursor = index + 1;
        while cursor < bytes.len() {
            if bytes[cursor] != 0 {
                end = cursor + 1;
            } else if cursor - end >= GAP {
                break;
            }
            cursor += 1;
        }
        out.push((start, bytes[start..end].to_vec()));
        index = end;
    }
    out
}

/// One emitted file.
#[derive(Debug, Clone)]
pub struct VectorFile {
    /// File stem, without the extension.
    pub name: String,
    /// The spec section it pins.
    pub section: String,
    /// What the file covers.
    pub description: String,
    /// Its cases.
    pub cases: Vec<Case>,
}

impl VectorFile {
    /// The canonical file bytes.
    pub fn json(&self) -> String {
        let cases: Vec<String> = self.cases.iter().map(Case::to_json).collect();
        format!(
            "{{\n  \"name\": \"{}\",\n  \"suite\": \"device-object-v2\",\n  \"kind\": \"storage\",\n  \"storage_format\": 1,\n  \"section\": \"{}\",\n  \"description\": \"{}\",\n  \"caseCount\": {},\n  \"cases\": [\n{}\n  ]\n}}\n",
            self.name,
            self.section,
            self.description,
            self.cases.len(),
            cases.join(",\n"),
        )
    }
}

fn file(name: &str, section: &str, description: &str, cases: Vec<Case>) -> VectorFile {
    VectorFile { name: name.to_string(), section: section.to_string(), description: description.to_string(), cases }
}

/// Every storage vector file, in emission order.
pub fn files() -> Vec<VectorFile> {
    vec![
        gate_file(),
        checkpoint_empty_file(),
        checkpoint_populated_file(),
        checkpoint_negative_file(),
        journal_kinds_file(),
        journal_removal_keys_file(),
        journal_negative_file(),
        work_slot_file(),
        ride_slot_file(),
        arm_handoff_file(),
        init_record_file(),
        resolution_file(),
        slot_stride_file(),
        terminal_result_file(),
        storage_claim_tag_file(),
    ]
}

/// §5.3's terminal-result table, one case per `(result type, encoded length)` pair, plus the
/// `DomainResult` codec the storage-local producers use and the length rule's negative twin.
fn terminal_result_file() -> VectorFile {
    let in_checkpoint = |entry: Vec<u8>| {
        let mut checkpoint = empty_checkpoint();
        checkpoint.result_start = 0;
        checkpoint.results = vec![entry];
        checkpoint.terminal_counter = 1;
        checkpoint.file(0)
    };
    let aborted_error_body = {
        // A diagnostic-text-free ErrorBody: category `semanticValidation`, detail 1.
        let mut body = zeros(48);
        u16_at(&mut body, 0, 14);
        u16_at(&mut body, 2, 1);
        body
    };
    let mut wrong_length = typed_result_entry(1, &OP_A, 1, 1, &[0x5a; 64]);
    u16_at(&mut wrong_length, 90, 88);

    file(
        "terminal-result",
        "OBC2_Storage_Format.md §5.3",
        "Every result type at its exact encoded length, the DomainResult codec, and the pairing rule that binds the two.",
        vec![
            Case::accept(
                "aborted-error-body-48",
                "Terminal state aborted, result type 0, a 48-byte ErrorBody with no diagnostic text.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_A, 2, 0, &aborted_error_body)),
            ),
            Case::accept(
                "object-result-64",
                "The ordinary publication result.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_A, 1, 1, &[0x5a; 64])),
            ),
            Case::accept(
                "draft-part-result-88",
                "The largest result, which is exactly the 88-byte body reservation.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_CHILD, 1, 2, &[0x6b; 88])),
            ),
            Case::accept(
                "abort-result-56",
                "AbortOperation's own result.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_ABORT, 1, 3, &[0x7c; 56])),
            ),
            Case::accept(
                "domain-result-weather-changed",
                "DomainResult outcome 1, weatherRequestChanged, with the domain-state revision equal to the new request-context revision.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_A, 1, 4, &domain_result_body(&OP_A, 4, 1, 3))),
            ),
            Case::accept(
                "domain-result-update-state-changed",
                "DomainResult outcome 2, updateStateChanged, with the new update repository Revision.",
                Subject::Checkpoint,
                0,
                in_checkpoint(typed_result_entry(1, &OP_INSTALL, 1, 4, &domain_result_body(&OP_INSTALL, 7, 2, 9))),
            ),
            Case::reject(
                "length-that-contradicts-its-type",
                "An ObjectResult declaring 88 bytes: the type fixes the length and the two must agree.",
                Subject::Checkpoint,
                0,
                in_checkpoint(wrong_length),
                "Overflow",
            ),
        ],
    )
}

/// §5.3's storage-internal claim tags, which are positives rather than rejections: they are
/// registered here and nowhere else, and a row carrying one is a perfectly ordinary active row.
fn storage_claim_tag_file() -> VectorFile {
    let claim = |opcode: u16, phase: u8| {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT,
            active: Some(active_entry(&OP_A, opcode, phase, 0x08)),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    file(
        "storage-claim-tags",
        "OBC2_Storage_Format.md §5.3",
        "The two 0xFF00-block claim tags a local producer stores, against the wire opcodes the other local producers store.",
        vec![
            Case::accept(
                "weather-context-change-ff01",
                "The device-local weather-context claim, in the reserved ninth row.",
                Subject::JournalSlot,
                0,
                claim(0xFF01, 5),
            ),
            Case::accept(
                "update-reconciliation-ff02",
                "The post-boot update-state reconciliation claim, also in the reserved row.",
                Subject::JournalSlot,
                0,
                claim(0xFF02, 5),
            ),
            Case::accept(
                "ride-publication-stores-start-upload",
                "A ride publication mirrors a wire operation and stores its opcode, 0x0100.",
                Subject::JournalSlot,
                0,
                claim(0x0100, 5),
            ),
            Case::accept(
                "map-import-stores-begin-draft",
                "A staged map import stores 0x0130 for its draft parent.",
                Subject::JournalSlot,
                0,
                claim(0x0130, 2),
            ),
            Case::accept(
                "map-import-child-stores-start-draft-part",
                "And 0x0131 for its single child.",
                Subject::JournalSlot,
                0,
                claim(0x0131, 1),
            ),
        ],
    )
}

fn gate_file() -> VectorFile {
    let body_crc = 0xDEAD_BEEFu32;
    let valid = gate(b"O2JG", 0, 7, 42, body_crc);
    let mut wrong_complement = valid.clone();
    wrong_complement[28] ^= 0x01;
    let crc = crc32_with_hole(&wrong_complement, 32);
    u32_at(&mut wrong_complement, 32, crc);
    let mut wrong_version = valid.clone();
    wrong_version[4] = 2;
    let crc = crc32_with_hole(&wrong_version, 32);
    u32_at(&mut wrong_version, 32, crc);
    let mut nonzero_tail = valid.clone();
    nonzero_tail[400] = 1;
    let crc = crc32_with_hole(&nonzero_tail, 32);
    u32_at(&mut nonzero_tail, 32, crc);
    let mut bad_gate_crc = valid.clone();
    bad_gate_crc[32] ^= 0xFF;

    file(
        "gate-sector",
        "OBC2_Storage_Format.md §4",
        "The one 512-byte gate layout every record shares, its invalidated form, and the checks that make it all-or-nothing.",
        vec![
            Case::accept("valid-journal-gate", "Magic, slot, scope, sequence, CRC and complement all agree.", Subject::Gate, 0, valid.clone()),
            Case::reject("invalidated-sector", "512 zero bytes: invalidation needs no sentinel because it fails magic and CRC.", Subject::Gate, 0, zeros(GATE_LEN), "Magic"),
            Case::reject("wrong-slot-index", "The same gate read at slot 1 is not a gate.", Subject::Gate, 1, valid.clone(), "SlotIndex"),
            Case::reject("complement-not-exact", "The one's complement copy of the body CRC is off by one bit.", Subject::Gate, 0, wrong_complement, "Complement"),
            Case::reject("unknown-version", "Format version 2 is not a version this build knows.", Subject::Gate, 0, wrong_version, "Version"),
            Case::reject("nonzero-reserved-tail", "Bytes 36..512 are reserved and must be zero when read.", Subject::Gate, 0, nonzero_tail, "Reserved"),
            Case::reject("gate-crc-mismatch", "The gate's own CRC does not cover its 512 bytes.", Subject::Gate, 0, bad_gate_crc, "GateCrc"),
        ],
    )
}

fn empty_checkpoint() -> Checkpoint {
    // §12's first checkpoint: epoch 1, through-sequence 0, next GenerationId 0, terminal counter 0,
    // and weather LogicalObjectId zero reserved by setting the weather repository's next candidate
    // to one while leaving the weather-state count zero.
    Checkpoint { epoch: 1, repositories: vec![repository_entry(4, 0, 1)], ..Checkpoint::default() }
}

fn checkpoint_empty_file() -> VectorFile {
    file(
        "checkpoint-first",
        "OBC2_Storage_Format.md §5, §12",
        "The first checkpoint a fresh card is born with, at its exact 65,536 bytes.",
        vec![Case::accept(
            "first-checkpoint",
            "Epoch 1, through-sequence 0, next GenerationId 0, weather logical ID zero reserved.",
            Subject::Checkpoint,
            0,
            empty_checkpoint().file(0),
        )],
    )
}

fn populated_checkpoint() -> Checkpoint {
    Checkpoint {
        epoch: 2,
        through_sequence: 9,
        next_generation: 93,
        terminal_counter: 3,
        repositories: vec![repository_entry(1, 4, 2), repository_entry(4, 0, 1), repository_entry(6, 2, 4)],
        heads: vec![head_entry(1, 3, None), head_entry(1, 9, None), head_entry(6, 1, Some(92))],
        actives: vec![active_entry(&OP_A, 0x0100, 3, 0x10), active_entry(&OP_PARENT, 0x0130, 2, 0x12)],
        parent: Some(parent_entry(1)),
        parts: vec![part_entry(1, 2, true), part_entry(2, 4, false)],
        retained: vec![retained_entry(40, 0b101, 2), retained_entry(41, 0b010, 0)],
        result_start: 62,
        results: vec![result_entry(1, &OP_A), result_entry(2, &OP_PARENT), result_entry(3, &OP_A)],
        handoff: Some(handoff_ref(4, 2)),
        weather: Some(weather_entry()),
        ride: Some(ride_entry()),
    }
}

/// §5.3's head entry with a full-width envelope: "the 96-byte envelope reservation is exactly the
/// catalog-projection ceiling of the metadata registry".
fn head_entry_full_envelope(kind: u16, id: u64) -> Vec<u8> {
    let mut out = head_entry(kind, id, None);
    u16_at(&mut out, 40, 96);
    for index in 0..96usize {
        out[48 + index] = (index as u8) | 1;
    }
    out
}

fn checkpoint_populated_file() -> VectorFile {
    file(
        "checkpoint-populated",
        "OBC2_Storage_Format.md §5.1, §5.3",
        "Every region occupied at once, including a result ring that wraps past index 63 and a manifest head carrying its resolution generation.",
        vec![
            Case::accept(
                "all-regions-occupied",
                "Three repositories, three heads, two actives, a draft parent with a sealed and a prepared part, two retained reasons, a wrapped result ring, a handoff, weather and an active ride.",
                Subject::Checkpoint,
                0,
                populated_checkpoint().file(0),
            ),
            Case::accept(
                "catalog-envelope-at-its-ceiling",
                "A head whose envelope fills the whole 96-byte reservation, which is the largest a registered schema may grow into. The bound reads as inclusive: 96 is a length the field can hold, not one past it.",
                Subject::Checkpoint,
                0,
                {
                    let mut checkpoint = empty_checkpoint();
                    checkpoint.heads = vec![head_entry_full_envelope(1, 3)];
                    checkpoint.file(0)
                },
            ),
            Case::accept(
                "draft-parent-manifest-streaming",
                "State 2: the parent-owned manifest is streaming.",
                Subject::Checkpoint,
                0,
                {
                    let mut checkpoint = empty_checkpoint();
                    checkpoint.parent = Some(parent_entry(2));
                    checkpoint.file(0)
                },
            ),
            Case::accept(
                "draft-parent-finalizing-with-its-resolution",
                "State 3, the only state in which the reserved resolution GenerationId is meaningful.",
                Subject::Checkpoint,
                0,
                {
                    let mut parent = parent_entry(3);
                    u64_at(&mut parent, 116, 92);
                    let mut checkpoint = empty_checkpoint();
                    checkpoint.parent = Some(parent);
                    checkpoint.file(0)
                },
            ),
            Case::accept(
                "draft-parent-aborting",
                "State 4: no new parts and no finalization.",
                Subject::Checkpoint,
                0,
                {
                    let mut checkpoint = empty_checkpoint();
                    checkpoint.parent = Some(parent_entry(4));
                    checkpoint.file(0)
                },
            ),
            Case::reject(
                "draft-parent-resolution-outside-finalizing",
                "The reserved resolution field is inactive zero in every state but finalizing.",
                Subject::Checkpoint,
                0,
                {
                    let mut parent = parent_entry(1);
                    u64_at(&mut parent, 116, 92);
                    let mut checkpoint = empty_checkpoint();
                    checkpoint.parent = Some(parent);
                    checkpoint.file(0)
                },
                "Reserved",
            ),
        ],
    )
}

fn checkpoint_negative_file() -> VectorFile {
    let base = populated_checkpoint();

    let mut unsorted = base.clone();
    unsorted.heads.swap(0, 1);
    let mut duplicate = base.clone();
    duplicate.heads[1] = head_entry(1, 3, None);

    // Both of these mutate a complete file, so the body CRC **and** the gate are rebuilt: the
    // declared reason has to be the rule that actually refuses first, not a stale checksum standing
    // in front of it.
    let mut over_capacity = base.clone().file(0);
    u16_at(&mut over_capacity, 50, 257);
    reseal_checkpoint(&mut over_capacity, 0);

    let mut nonzero_tail = base.clone().file(0);
    nonzero_tail[60_200] = 1;
    reseal_checkpoint(&mut nonzero_tail, 0);

    let mut torn_body = base.clone().file(0);
    torn_body[600] ^= 0xFF;

    // Nine rows fit only as "eight normal plus the one reserved". Ten normal rows is the count
    // rule; nine normal rows plus the reserved one is the *composition* rule, and they are
    // different refusals of the same region.
    let mut ten_actives = base.clone();
    ten_actives.actives = (0..10)
        .map(|index| {
            let mut operation = OP_A;
            operation[0] = index;
            active_entry(&operation, 0x0100, 3, 0)
        })
        .collect();

    let mut nine_normal_rows = base.clone();
    nine_normal_rows.actives = (0..9)
        .map(|index| {
            let mut operation = OP_A;
            operation[0] = index;
            // No reserved-slot flag on any of them: nine *normal* claims, which §5.2 forbids.
            active_entry(&operation, 0x0100, 3, 0)
        })
        .collect();

    let mut eight_plus_reserved = base.clone();
    eight_plus_reserved.actives = (0..9)
        .map(|index| {
            let mut operation = OP_A;
            operation[0] = index;
            let flags = if index == 8 { 0x08 } else { 0 };
            active_entry(&operation, 0x0100, 3, flags)
        })
        .collect();

    file(
        "checkpoint-negative",
        "OBC2_Storage_Format.md §5.1, §5.2, §5.3",
        "Every structural rule a checkpoint body must satisfy, each violated exactly once.",
        vec![
            Case::reject(
                "heads-out-of-order",
                "Occupied entries must be sorted by their stated key.",
                Subject::Checkpoint,
                0,
                unsorted.file(0),
                "Order",
            ),
            Case::reject(
                "duplicate-head-key",
                "Two entries share (kind, logical id).",
                Subject::Checkpoint,
                0,
                duplicate.file(0),
                "Duplicate",
            ),
            Case::reject(
                "head-count-above-capacity",
                "A count above its region capacity is refused before any derived offset is used.",
                Subject::Checkpoint,
                0,
                over_capacity,
                "Count",
            ),
            Case::reject(
                "nonzero-region-tail",
                "The run between the last region and the body CRC is reserved.",
                Subject::Checkpoint,
                0,
                nonzero_tail,
                "Reserved",
            ),
            Case::reject(
                "torn-body",
                "One flipped byte breaks the body CRC.",
                Subject::Checkpoint,
                0,
                torn_body,
                "BodyCrc",
            ),
            Case::reject(
                "ten-active-rows",
                "The region holds nine rows: a tenth is above its capacity.",
                Subject::Checkpoint,
                0,
                ten_actives.file(0),
                "Count",
            ),
            Case::reject(
                "nine-normal-active-rows",
                "Nine rows fit, but only as eight normal claims plus the one reserved cancellation/recovery row.",
                Subject::Checkpoint,
                0,
                nine_normal_rows.file(0),
                "Count",
            ),
            Case::accept(
                "eight-normal-rows-plus-the-reserved-one",
                "The composition that does fit: eight ordinary claims and one row carrying flag bit 3.",
                Subject::Checkpoint,
                0,
                eight_plus_reserved.file(0),
            ),
        ],
    )
}

const P_ACTIVE_PUT: u32 = 1 << 0;
const P_ACTIVE_REMOVE: u32 = 1 << 1;
const P_HEAD_PUT: u32 = 1 << 2;
const P_HEAD_REMOVE: u32 = 1 << 3;
const P_PARENT_PUT: u32 = 1 << 4;
const P_PART_PUT: u32 = 1 << 6;
const P_PARENT_REMOVE: u32 = 1 << 5;
const P_PART_REMOVE: u32 = 1 << 7;
const P_PREVIOUS_PUT: u32 = 1 << 8;
const P_PREVIOUS_REMOVE: u32 = 1 << 9;
const P_RESULT_APPEND: u32 = 1 << 10;
const P_HANDOFF_PUT: u32 = 1 << 11;
const P_HANDOFF_REMOVE: u32 = 1 << 12;
const P_REPOSITORY_REVISION: u32 = 1 << 13;
const P_REPOSITORY_CURSOR: u32 = 1 << 14;
const P_WEATHER_PUT: u32 = 1 << 15;
const P_RIDE_PUT: u32 = 1 << 16;
const P_RIDE_REMOVE: u32 = 1 << 17;
const P_GENERATION_CURSOR: u32 = 1 << 18;

fn claim_record() -> Vec<u8> {
    let mutation = Mutation {
        presence: P_ACTIVE_PUT | P_GENERATION_CURSOR,
        generation_cursor: 43,
        active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
        ..Mutation::default()
    };
    journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
}

fn terminal_record() -> Vec<u8> {
    let mut removal = zeros(128);
    bytes_at(&mut removal, 0, &OP_A);
    let mutation = Mutation {
        presence: P_ACTIVE_REMOVE | P_HEAD_PUT | P_RESULT_APPEND | P_REPOSITORY_REVISION,
        repository_kind: 1,
        repository_revision: 9,
        active: Some(removal),
        head: Some(head_entry(1, 7, None)),
        result: Some(result_entry(1, &OP_A)),
        ..Mutation::default()
    };
    journal_slot(1, 2, 1, 3, Some(&OP_A), &mutation)
}

fn journal_kinds_file() -> VectorFile {
    let work = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT,
            active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
            ..Mutation::default()
        };
        journal_slot(1, 3, 2, 2, Some(&OP_A), &mutation)
    };
    let retention = {
        let mut removal = zeros(64);
        removal[0] = 1;
        u64_at(&mut removal, 16, 40);
        let mutation = Mutation { presence: P_PREVIOUS_REMOVE, retained: Some(removal), ..Mutation::default() };
        journal_slot(1, 4, 3, 4, None, &mutation)
    };
    let handoff = {
        let mutation = Mutation { presence: P_HANDOFF_PUT, handoff: Some(handoff_ref(4, 2)), ..Mutation::default() };
        journal_slot(1, 5, 4, 5, Some(&OP_INSTALL), &mutation)
    };
    let handoff_cleanup = {
        let mutation = Mutation { presence: P_HANDOFF_REMOVE, ..Mutation::default() };
        journal_slot(1, 6, 5, 5, None, &mutation)
    };
    let domain = {
        let mutation = Mutation {
            presence: P_RIDE_PUT | P_GENERATION_CURSOR,
            generation_cursor: 78,
            ride: Some(ride_entry()),
            ..Mutation::default()
        };
        journal_slot(1, 7, 6, 6, None, &mutation)
    };
    let weather_publication = {
        let mut removal = zeros(128);
        bytes_at(&mut removal, 0, &OP_A);
        let mutation = Mutation {
            presence: P_ACTIVE_REMOVE
                | P_HEAD_PUT
                | P_RESULT_APPEND
                | P_WEATHER_PUT
                | P_PREVIOUS_PUT
                | P_REPOSITORY_REVISION,
            repository_kind: 4,
            repository_revision: 12,
            active: Some(removal),
            head: Some(head_entry(4, 0, None)),
            result: Some(result_entry(2, &OP_A)),
            retained: Some(retained_entry(40, 0b100, 0)),
            weather: Some(weather_entry()),
            ..Mutation::default()
        };
        journal_slot(1, 8, 7, 3, Some(&OP_A), &mutation)
    };
    let draft_claim = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT | P_PARENT_PUT | P_PART_PUT,
            active: Some(active_entry(&OP_PARENT, 0x0130, 2, 0x12)),
            parent: Some(parent_entry(1)),
            part: Some(part_entry(1, 4, false)),
            ..Mutation::default()
        };
        journal_slot(1, 9, 8, 1, Some(&OP_PARENT), &mutation)
    };

    file(
        "journal-record-kinds",
        "OBC2_Storage_Format.md §6.1",
        "One record of every kind: claim, work, terminal, retention, handoff, zero-identity handoff cleanup, and the pre-claim ride domain record.",
        vec![
            Case::accept("claim", "Active put plus the generation reservation the claim carries.", Subject::JournalSlot, 0, claim_record()),
            Case::accept("work", "Refreshes an existing claim's durable-progress facts.", Subject::JournalSlot, 2, work),
            Case::accept("terminal-publication", "Active remove, head put, repository revision and result append in one record.", Subject::JournalSlot, 1, terminal_record()),
            Case::accept("retention-remove", "Zero identity, exactly one previous entry changed.", Subject::JournalSlot, 3, retention),
            Case::accept("handoff-put", "The armed projection under the install claim.", Subject::JournalSlot, 4, handoff),
            Case::accept("handoff-cleanup", "The zero-identity removal suffix of a complete handoff.", Subject::JournalSlot, 5, handoff_cleanup),
            Case::accept("ride-domain-record", "The pre-claim ride record that reserves its generation.", Subject::JournalSlot, 6, domain),
            Case::accept("weather-publication", "Head, result, weather state and the domain-retention entry in one terminal record.", Subject::JournalSlot, 7, weather_publication),
            Case::accept("draft-claim", "A parent claim that atomically puts the parent and its first prepared part.", Subject::JournalSlot, 8, draft_claim),
        ],
    )
}

/// Offsets of the mutation's entry slots, for the removal cases below.
fn at_active() -> usize {
    40
}
fn at_parent() -> usize {
    328
}
fn at_part() -> usize {
    456
}
fn at_previous() -> usize {
    552
}

/// One terminal record whose only interesting content is the removal entry under test.
///
/// A removal never travels alone: §6.1 requires a terminal record to remove its active row and
/// append its result, so each case is wrapped in the smallest record that admits it.
fn removal_record(presence: u32, offset: usize, entry: Vec<u8>) -> Vec<u8> {
    let mut active_removal = zeros(128);
    bytes_at(&mut active_removal, 0, &OP_A);
    let mut mutation = Mutation {
        presence: P_ACTIVE_REMOVE | P_RESULT_APPEND | presence,
        active: Some(active_removal),
        result: Some(result_entry(1, &OP_A)),
        ..Mutation::default()
    };
    match offset {
        40 => mutation.active = Some(entry),
        328 => mutation.parent = Some(entry),
        456 => mutation.part = Some(entry),
        552 => mutation.retained = Some(entry),
        _ => unreachable!("no other entry offset has a removal case"),
    }
    journal_slot(1, 2, 1, 3, Some(&OP_A), &mutation)
}

fn journal_removal_keys_file() -> VectorFile {
    let head_removal = |mutate: fn(&mut Vec<u8>)| {
        let mut entry = zeros(160);
        entry[0] = 1;
        u16_at(&mut entry, 2, 1);
        u64_at(&mut entry, 4, 7);
        mutate(&mut entry);
        let mut removal = zeros(128);
        bytes_at(&mut removal, 0, &OP_A);
        let mutation = Mutation {
            presence: P_ACTIVE_REMOVE | P_HEAD_REMOVE | P_RESULT_APPEND,
            active: Some(removal),
            head: Some(entry),
            result: Some(result_entry(1, &OP_A)),
            ..Mutation::default()
        };
        journal_slot(1, 2, 1, 3, Some(&OP_A), &mutation)
    };

    let mut ride_removal_entry = zeros(128);
    ride_removal_entry[0] = 1;
    let ride_removal = {
        let mutation =
            Mutation { presence: P_RIDE_REMOVE, ride: Some(ride_removal_entry.clone()), ..Mutation::default() };
        journal_slot(1, 3, 2, 6, None, &mutation)
    };
    let mut ride_extra = ride_removal_entry.clone();
    ride_extra[64] = 1;
    let ride_removal_extra = {
        let mutation = Mutation { presence: P_RIDE_REMOVE, ride: Some(ride_extra), ..Mutation::default() };
        journal_slot(1, 3, 2, 6, None, &mutation)
    };
    let mut ride_unoccupied = zeros(128);
    ride_unoccupied[1] = 1;
    let ride_removal_unoccupied = {
        let mutation = Mutation { presence: P_RIDE_REMOVE, ride: Some(ride_unoccupied), ..Mutation::default() };
        journal_slot(1, 3, 2, 6, None, &mutation)
    };

    file(
        "journal-removal-keys",
        "OBC2_Storage_Format.md §6.1",
        "A removal carries only key bytes, and the occupied byte where the entry shape has one.",
        vec![
            Case::accept(
                "head-removal",
                "Occupied byte plus kind and logical id, everything else zero.",
                Subject::JournalSlot,
                1,
                head_removal(|_| {}),
            ),
            Case::reject(
                "head-removal-with-payload-byte",
                "One nonzero byte outside the key ranges.",
                Subject::JournalSlot,
                1,
                head_removal(|entry| entry[20] = 1),
                "KeyBytes",
            ),
            Case::reject(
                "head-removal-without-occupied",
                "The occupied byte must still be 1 so an all-zero region always means absent.",
                Subject::JournalSlot,
                1,
                head_removal(|entry| entry[0] = 0),
                "Occupied",
            ),
            Case::accept(
                "active-ride-removal",
                "The singleton removal carries only the occupied byte.",
                Subject::JournalSlot,
                2,
                ride_removal,
            ),
            Case::reject(
                "active-ride-removal-with-extra-byte",
                "Any other nonzero byte is not a removal.",
                Subject::JournalSlot,
                2,
                ride_removal_extra,
                "KeyBytes",
            ),
            Case::accept(
                "active-operation-removal",
                "Sixteen key bytes and nothing else — this entry shape has no occupied byte.",
                Subject::JournalSlot,
                1,
                removal_record(P_ACTIVE_REMOVE, at_active(), {
                    let mut entry = zeros(128);
                    bytes_at(&mut entry, 0, &OP_A);
                    entry
                }),
            ),
            Case::reject(
                "active-operation-removal-with-payload-byte",
                "A nonzero byte outside the key range.",
                Subject::JournalSlot,
                1,
                removal_record(P_ACTIVE_REMOVE, at_active(), {
                    let mut entry = zeros(128);
                    bytes_at(&mut entry, 0, &OP_A);
                    entry[84] = 3;
                    entry
                }),
                "KeyBytes",
            ),
            Case::accept(
                "draft-parent-removal",
                "The parent's own 16 key bytes.",
                Subject::JournalSlot,
                1,
                removal_record(P_PARENT_REMOVE, at_parent(), {
                    let mut entry = zeros(128);
                    bytes_at(&mut entry, 0, &OP_PARENT);
                    entry
                }),
            ),
            Case::accept(
                "draft-part-removal",
                "Parent, DraftPartKind and part key; the child operation and every payload field are zero.",
                Subject::JournalSlot,
                1,
                removal_record(P_PART_REMOVE, at_part(), {
                    let mut entry = zeros(96);
                    bytes_at(&mut entry, 0, &OP_PARENT);
                    u16_at(&mut entry, 48, 1);
                    u64_at(&mut entry, 52, 1);
                    entry
                }),
            ),
            Case::reject(
                "draft-part-removal-with-a-generation",
                "The generation is not a key byte.",
                Subject::JournalSlot,
                1,
                removal_record(P_PART_REMOVE, at_part(), {
                    let mut entry = zeros(96);
                    bytes_at(&mut entry, 0, &OP_PARENT);
                    u16_at(&mut entry, 48, 1);
                    u64_at(&mut entry, 52, 1);
                    u64_at(&mut entry, 60, 91);
                    entry
                }),
                "KeyBytes",
            ),
            Case::accept(
                "retained-previous-removal",
                "The occupied byte and the generation key.",
                Subject::JournalSlot,
                1,
                removal_record(P_PREVIOUS_REMOVE, at_previous(), {
                    let mut entry = zeros(64);
                    entry[0] = 1;
                    u64_at(&mut entry, 16, 40);
                    entry
                }),
            ),
            Case::reject(
                "retained-previous-removal-with-reasons",
                "The reason flags are not key bytes: a removal names the entry, it does not describe it.",
                Subject::JournalSlot,
                1,
                removal_record(P_PREVIOUS_REMOVE, at_previous(), {
                    let mut entry = zeros(64);
                    entry[0] = 1;
                    entry[1] = 0b001;
                    u64_at(&mut entry, 16, 40);
                    entry
                }),
                "KeyBytes",
            ),
            Case::accept(
                "update-handoff-removal",
                "The singleton removal is all 240 bytes zero; the presence bit is what distinguishes it from absence.",
                Subject::JournalSlot,
                1,
                {
                    let mutation = Mutation { presence: P_HANDOFF_REMOVE, ..Mutation::default() };
                    journal_slot(1, 2, 1, 5, None, &mutation)
                },
            ),
            Case::reject(
                "active-ride-removal-unoccupied",
                "Occupied must be exactly 1.",
                Subject::JournalSlot,
                2,
                ride_removal_unoccupied,
                "Occupied",
            ),
        ],
    )
}

fn journal_negative_file() -> VectorFile {
    let claim_with_head = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT | P_HEAD_PUT,
            active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
            head: Some(head_entry(1, 7, None)),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    let terminal_without_result = {
        let mut removal = zeros(128);
        bytes_at(&mut removal, 0, &OP_A);
        let mutation = Mutation { presence: P_ACTIVE_REMOVE, active: Some(removal), ..Mutation::default() };
        journal_slot(1, 2, 1, 3, Some(&OP_A), &mutation)
    };
    let retention_with_identity = {
        let mut removal = zeros(64);
        removal[0] = 1;
        u64_at(&mut removal, 16, 40);
        let mutation = Mutation { presence: P_PREVIOUS_REMOVE, retained: Some(removal), ..Mutation::default() };
        journal_slot(1, 4, 3, 4, Some(&OP_A), &mutation)
    };
    let weather_in_a_claim = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT | P_WEATHER_PUT,
            active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
            weather: Some(weather_entry()),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    let cursor_on_a_terminal = {
        let mut removal = zeros(128);
        bytes_at(&mut removal, 0, &OP_A);
        let mutation = Mutation {
            presence: P_ACTIVE_REMOVE | P_RESULT_APPEND | P_GENERATION_CURSOR,
            generation_cursor: 50,
            active: Some(removal),
            result: Some(result_entry(1, &OP_A)),
            ..Mutation::default()
        };
        journal_slot(1, 2, 1, 3, Some(&OP_A), &mutation)
    };
    let put_and_remove = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT | P_ACTIVE_REMOVE,
            active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    let undefined_bit = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT | (1 << 19),
            active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    let unregistered_opcode = {
        let mutation = Mutation {
            presence: P_ACTIVE_PUT,
            active: Some(active_entry(&OP_A, 0x0999, 3, 0x10)),
            ..Mutation::default()
        };
        journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
    };
    let wrong_slot = journal_slot(
        1,
        1,
        3,
        1,
        Some(&OP_A),
        &Mutation { presence: P_ACTIVE_PUT, active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)), ..Mutation::default() },
    );
    let mut nonzero_pad = claim_record();
    nonzero_pad[3_000] = 1;

    file(
        "journal-negative",
        "OBC2_Storage_Format.md §6.1",
        "The per-kind combination rules, the presence-bit rules, and the storage-internal claim-tag registry.",
        vec![
            Case::reject(
                "claim-that-mutates-a-head",
                "A claim record forbids head mutation.",
                Subject::JournalSlot,
                0,
                claim_with_head,
                "Combination",
            ),
            Case::reject(
                "terminal-without-result",
                "A terminal record requires active remove and result append.",
                Subject::JournalSlot,
                1,
                terminal_without_result,
                "Combination",
            ),
            Case::reject(
                "retention-with-an-identity",
                "A retention record has zero OperationId and digest.",
                Subject::JournalSlot,
                3,
                retention_with_identity,
                "Combination",
            ),
            Case::reject(
                "weather-outside-a-terminal",
                "Weather state changes only in a terminal record.",
                Subject::JournalSlot,
                0,
                weather_in_a_claim,
                "Combination",
            ),
            Case::reject(
                "generation-cursor-on-a-terminal",
                "No record but the four named carriers may set presence bit 18.",
                Subject::JournalSlot,
                1,
                cursor_on_a_terminal,
                "Combination",
            ),
            Case::reject(
                "put-and-remove-together",
                "Put and remove for one entry are mutually exclusive.",
                Subject::JournalSlot,
                0,
                put_and_remove,
                "Combination",
            ),
            Case::reject(
                "undefined-presence-bit",
                "Bits 19..31 are zero.",
                Subject::JournalSlot,
                0,
                undefined_bit,
                "Reserved",
            ),
            Case::reject(
                "unregistered-opcode",
                "An active row's opcode is a registered wire opcode or a registered claim tag.",
                Subject::JournalSlot,
                0,
                unregistered_opcode,
                "UnknownEnum",
            ),
            Case::reject(
                "wrong-physical-slot",
                "The body's own slot index must equal the physical slot it was read from. Section 6.3's sequence-to-slot mapping is a separate replay rule.",
                Subject::JournalSlot,
                0,
                wrong_slot,
                "SlotIndex",
            ),
            Case::accept(
                "repository-logical-id-cursor",
                "Presence bit 14 alone: the repository's next logical-ID candidate and its exhausted flag move without its revision.",
                Subject::JournalSlot,
                0,
                {
                    let mutation = Mutation {
                        presence: P_ACTIVE_PUT | P_REPOSITORY_CURSOR,
                        repository_kind: 1,
                        next_logical_id: 9,
                        active: Some(active_entry(&OP_A, 0x0100, 3, 0x10)),
                        ..Mutation::default()
                    };
                    journal_slot(1, 1, 0, 1, Some(&OP_A), &mutation)
                },
            ),
            Case::reject(
                "nonzero-slot-pad",
                "The pad to the 16,384-byte stride is reserved.",
                Subject::JournalSlot,
                0,
                nonzero_pad,
                "Reserved",
            ),
        ],
    )
}

fn work_slot_file() -> VectorFile {
    let streaming = small_slot(work_body(1, 4_096, 1, 4_096), b"O2WG", 0, 42, 1);
    let sealed = small_slot(work_body(2, 65_536, 2, 65_536), b"O2WG", 1, 42, 2);
    let unreachable = small_slot(work_body(3, 32_768, 1, 4_096), b"O2WG", 0, 42, 3);
    let mut over_declared = work_body(4, 65_537, 1, 4_096);
    u64_at(&mut over_declared, 128, 65_537);
    let over_declared = small_slot(over_declared, b"O2WG", 0, 42, 4);

    file(
        "work-slot",
        "OBC2_Storage_Format.md §7",
        "The WORK slot both device profiles can hold, including the observed-length field the mandatory rewind is decided from.",
        vec![
            Case::accept("streaming", "A durable checkpoint at offset 4,096.", Subject::WorkSlot, 0, streaming),
            Case::accept("sealed", "The one durable work fact the restart-only profile writes.", Subject::WorkSlot, 1, sealed),
            Case::accept("offset-above-observed-length", "A valid slot whose durable offset the payload cannot reach: recovery skips it as if invalid and rewinds.", Subject::WorkSlot, 0, unreachable),
            Case::reject("offset-above-declared-length", "The durable offset cannot exceed the declared length.", Subject::WorkSlot, 0, over_declared, "Overflow"),
        ],
    )
}

fn ride_slot_file() -> VectorFile {
    let recording = small_slot(ride_body(0, 4_096, 1, 4_096), b"O2RG", 0, 77, 0);
    let wrapped = small_slot(ride_body(17, 8_192, 1, 8_192), b"O2RG", 1, 77, 17);
    let mut sealed = ride_body(18, 12_288, 3, 12_288);
    u64_at(&mut sealed, 120, 12_288);
    u32_at(&mut sealed, 128, 0x4444_5555);
    let sealed = small_slot(sealed, b"O2RG", 2, 77, 18);
    let mut premature_seal = ride_body(1, 4_096, 1, 4_096);
    u64_at(&mut premature_seal, 120, 4_096);
    let premature_seal = small_slot(premature_seal, b"O2RG", 1, 77, 1);

    file(
        "ride-slot",
        "OBC2_Storage_Format.md §7.1",
        "The 16-slot ride ring: its position rule, its sealed facts, and the observed length the same rewind applies to.",
        vec![
            Case::accept("recording", "The initial checkpoint at ring position 0.", Subject::RideSlot, 0, recording),
            Case::accept("ring-wrap", "Sequence 17 lives at position 1, because slots are written at sequence mod 16.", Subject::RideSlot, 1, wrapped),
            Case::accept("sealed", "Final length and CRC, which the state can never claim without a matching slot.", Subject::RideSlot, 2, sealed),
            Case::reject("sealed-facts-before-seal", "Sealed length and CRC are inactive zero before seal.", Subject::RideSlot, 1, premature_seal, "Reserved"),
        ],
    )
}

fn arm_handoff_file() -> VectorFile {
    let prepared = small_slot(arm_body(4, 1), b"O2HG", 0, 4, 1);
    let armed = small_slot(arm_body(4, 2), b"O2HG", 1, 4, 2);
    let mut rollback = arm_body(4, 2);
    u16_at(&mut rollback, 64 + 10, 1);
    u64_at(&mut rollback, 64 + 200, 55);
    u64_at(&mut rollback, 64 + 208, 131_072);
    u32_at(&mut rollback, 64 + 216, 0x1357_9bdf);
    let rollback = small_slot(rollback, b"O2HG", 0, 4, 2);
    let mut zero_arm_generation = arm_body(4, 1);
    u32_at(&mut zero_arm_generation, 64 + 84, 0);
    let zero_arm_generation = small_slot(zero_arm_generation, b"O2HG", 0, 4, 1);
    let mut sequence_mismatch = arm_body(4, 1);
    u64_at(&mut sequence_mismatch, 24, 5);
    let sequence_mismatch = small_slot(sequence_mismatch, b"O2HG", 0, 5, 1);

    file(
        "arm-handoff",
        "OBC2_Storage_Format.md §10",
        "The alternating update handoff, whose gate scope is the handoff sequence and whose gate sequence is the phase.",
        vec![
            Case::accept("prepared", "The package and snapshot are pinned; the OBCU page is not written yet.", Subject::ArmFile, 0, prepared),
            Case::accept("armed", "The strictly greater pair of the same handoff sequence.", Subject::ArmFile, 1, armed),
            Case::accept("armed-with-rollback-snapshot", "The rollback fields are valid exactly when flag bit 0 is set.", Subject::ArmFile, 0, rollback),
            Case::reject("zero-arm-generation", "The OBCU arm generation is nonzero.", Subject::ArmFile, 0, zero_arm_generation, "Reserved"),
            Case::reject("sequence-mismatch", "The HandoffRef sequence must equal the outer body and the gate scope.", Subject::ArmFile, 0, sequence_mismatch, "Sequence"),
        ],
    )
}

fn init_record_file() -> VectorFile {
    let witness = small_slot(
        init_body(),
        b"O2IG",
        0,
        u64::from_le_bytes(STORE[..8].try_into().unwrap()),
        u64::from_le_bytes(STORE[8..].try_into().unwrap()),
    );
    let foreign = {
        let mut body = init_body();
        bytes_at(&mut body, 8, &[0x11; 16]);
        let mut slot = witness.clone();
        let crc = crc32_with_hole(&body, SMALL_BODY_CRC_OFFSET);
        u32_at(&mut body, SMALL_BODY_CRC_OFFSET, crc);
        bytes_at(&mut slot, 0, &body);
        slot
    };

    file(
        "init-record",
        "OBC2_Storage_Format.md §12",
        "The incomplete-initialization witness, whose gate carries the StoreId's own bytes as scope and sequence so the two records are bound.",
        vec![
            Case::accept("witness", "A 512-byte body and its O2IG gate at file offset 512.", Subject::InitRecord, 0, witness),
            Case::reject("body-from-another-store", "The gate names this StoreId, so a body carrying another one does not validate under it.", Subject::InitRecord, 0, foreign, "BodyCrc"),
        ],
    )
}

fn resolution_file() -> VectorFile {
    let one = resolution_body(&[(PART_REF, 91)]);
    let full: Vec<([u8; 16], u64)> = (0..32u64)
        .map(|index| {
            let mut part_ref = [0u8; 16];
            part_ref[0] = index as u8;
            (part_ref, 100 + index)
        })
        .collect();
    let maximum = resolution_body(&full);
    let mut unordered = resolution_body(&[(PART_REF, 91), ([0x01; 16], 92)]);
    let mut truncated = resolution_body(&[(PART_REF, 91), ([0x7f; 16], 92)]);
    truncated.truncate(truncated.len() - 1);
    // Two separate single-fault cases: a count of zero in a body whose length agrees with it, and
    // a count above the 32-child maximum in a body whose length agrees with *that*. Mutating only
    // the count of a longer body would trip the length rule first and prove nothing about the
    // count bound.
    let zero_count = {
        let mut out = zeros(8);
        u32_at(&mut out, 0, 0);
        out
    };
    let over_maximum = {
        let mut out = zeros(8 + 33 * 24);
        u32_at(&mut out, 0, 33);
        for index in 0..33usize {
            out[8 + index * 24] = index as u8;
            u64_at(&mut out, 8 + index * 24 + 16, 100 + index as u64);
        }
        out
    };
    let mut duplicate = resolution_body(&[(PART_REF, 91), (PART_REF, 92)]);
    u32_at(&mut duplicate, 0, 2);
    u32_at(&mut unordered, 0, 2);

    file(
        "resolution-generation",
        "OBC2_Storage_Format.md §8",
        "The store-private table that resolves a published manifest's DraftPartRefs, at 32 bytes for one child and 776 for the maximum.",
        vec![
            Case::accept("one-entry", "The smallest table: eight header bytes and one 24-byte entry.", Subject::Resolution, 0, one),
            Case::accept("thirty-two-entries", "The 32-child maximum, 776 bytes, which bounds a full reachability pass.", Subject::Resolution, 0, maximum),
            Case::reject(
                "count-zero",
                "The count is 1 through 32; an eight-byte body declaring zero entries is the smallest way to violate it.",
                Subject::Resolution,
                0,
                zero_count,
                "Count",
            ),
            Case::reject(
                "count-above-the-maximum",
                "Thirty-three entries, in a body whose length agrees with the count, so the bound is what refuses it.",
                Subject::Resolution,
                0,
                over_maximum,
                "Count",
            ),
            Case::reject("truncated-body", "A cut during the one-shot write leaves a body whose count and length disagree.", Subject::Resolution, 0, truncated, "Length"),
            Case::reject("entries-out-of-order", "Entries are ordered by DraftPartRef bytes, compared lexicographically.", Subject::Resolution, 0, unordered, "Order"),
            Case::reject("duplicate-reference", "Refs are unique.", Subject::Resolution, 0, duplicate, "Duplicate"),
        ],
    )
}

fn slot_stride_file() -> VectorFile {
    let mut journal = claim_record();
    journal[SLOT_STRIDE - 1] = 1;
    let mut work = small_slot(work_body(1, 4_096, 1, 4_096), b"O2WG", 0, 42, 1);
    work[SLOT_STRIDE - 1] = 1;
    let mut ride = small_slot(ride_body(0, 4_096, 1, 4_096), b"O2RG", 0, 77, 0);
    ride[2_000] = 1;
    let mut arm = small_slot(arm_body(4, 1), b"O2HG", 0, 4, 1);
    arm[SLOT_FILE_LEN - 1] = 1;
    let mut init = small_slot(
        init_body(),
        b"O2IG",
        0,
        u64::from_le_bytes(STORE[..8].try_into().unwrap()),
        u64::from_le_bytes(STORE[8..].try_into().unwrap()),
    );
    init[9_000] = 1;

    file(
        "slot-strides",
        "OBC2_Storage_Format.md §1.1, §6, §7, §7.1, §10, §12",
        "Every slot is one 16,384-byte program page and its pad is zero; a nonzero pad is rejected, which is what makes a torn page detectable.",
        vec![
            Case::reject("journal-pad", "A journal slot's 14,336-byte pad.", Subject::JournalSlot, 0, journal, "Reserved"),
            Case::reject("work-pad", "A WORK slot's 15,360-byte pad.", Subject::WorkSlot, 0, work, "Reserved"),
            Case::reject("ride-pad", "A RIDE.ACT slot's pad.", Subject::RideSlot, 0, ride, "Reserved"),
            Case::reject("arm-pad", "An ARM file's pad.", Subject::ArmFile, 0, arm, "Reserved"),
            Case::reject("init-pad", "INIT.REC's pad.", Subject::InitRecord, 0, init, "Reserved"),
        ],
    )
}

// ---------------------------------------------------------------------------------------------
// Crash-cut transcripts
// ---------------------------------------------------------------------------------------------

/// One media operation of a commit path, in the order its section prescribes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The OBC2 file it addresses.
    pub file: &'static str,
    /// `"write"` or `"sync"`.
    pub kind: &'static str,
    /// The byte offset a write addresses; zero for a sync.
    pub offset: usize,
    /// The byte count a write requests; zero for a sync.
    pub length: usize,
    /// What the step is for.
    pub note: &'static str,
}

/// One commit path and the states a cut anywhere in it may recover.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// Stable name.
    pub name: &'static str,
    /// The section that fixes the ordering.
    pub section: &'static str,
    /// What the path does.
    pub description: &'static str,
    /// The ordered operations.
    pub steps: &'static [Step],
    /// The states a reboot after any cut may produce, named.
    pub outcomes: &'static [&'static str],
    /// Anything a reader must know about how normative this sequence is. Empty when the ordering is
    /// exactly what the spec fixes.
    pub note: &'static str,
}

const fn write(file: &'static str, offset: usize, length: usize, note: &'static str) -> Step {
    Step { file, kind: "write", offset, length, note }
}

const fn sync(file: &'static str, note: &'static str) -> Step {
    Step { file, kind: "sync", offset: 0, length: 0, note }
}

const JOURNAL_APPEND: [Step; 4] = [
    write("COMMIT.JNL", 0, SLOT_STRIDE, "the whole stride: body, a zeroed gate sector, and a zeroed pad"),
    sync("COMMIT.JNL", "the body is durable and the slot carries no valid gate"),
    write("COMMIT.JNL", JOURNAL_GATE_OFFSET, 512, "the gate that makes the body a record"),
    sync("COMMIT.JNL", "the commit point"),
];

const COMPACTION: [Step; 10] = [
    write("CAT1.CHK", CHECKPOINT_GATE_OFFSET, 512, "invalidate the inactive checkpoint's gate"),
    sync("CAT1.CHK", "the inactive checkpoint is now unusable and its body may be reused"),
    write("CAT1.CHK", 0, CHECKPOINT_BODY_LEN, "the complete body at epoch E + 1 and through-sequence S"),
    sync("CAT1.CHK", "the body is durable; the active checkpoint is still the selected one"),
    write("CAT1.CHK", CHECKPOINT_GATE_OFFSET, 512, "the O2CG gate"),
    sync("CAT1.CHK", "the new checkpoint is selected and every old-epoch slot is inert"),
    write("COMMIT.JNL", 0, SLOT_STRIDE, "only now, sequence S + 1 at slot zero of epoch E + 1"),
    sync("COMMIT.JNL", "its body"),
    write("COMMIT.JNL", JOURNAL_GATE_OFFSET, 512, "its gate"),
    sync("COMMIT.JNL", "the first commit of the new epoch"),
];

const WORK_SEAL: [Step; 6] = [
    write("WORK", SLOT_STRIDE + SMALL_GATE_OFFSET, 512, "invalidate the older slot's gate"),
    sync("WORK", "the slot about to be reused is unusable; the current record is untouched"),
    write("WORK", SLOT_STRIDE, SLOT_STRIDE, "the sealed body, with a zeroed gate sector and pad"),
    sync("WORK", "the body is durable"),
    write("WORK", SLOT_STRIDE + SMALL_GATE_OFFSET, 512, "the O2WG gate"),
    sync("WORK", "seal is durable and domain validation may run"),
];

const ARM_ADVANCE: [Step; 6] = [
    write("ARM1.HND", SMALL_GATE_OFFSET, 512, "invalidate the older/inactive ARM gate"),
    sync("ARM1.HND", "the selected ARM file remains valid throughout"),
    write("ARM1.HND", 0, SLOT_FILE_LEN, "the next phase's body, with a zeroed gate sector and pad"),
    sync("ARM1.HND", "the body is durable"),
    write("ARM1.HND", SMALL_GATE_OFFSET, 512, "the O2HG gate"),
    sync("ARM1.HND", "the strictly greater (sequence, phase) pair is now selected"),
];

const MANIFEST_PUBLICATION: [Step; 6] = [
    write("GEN", 0, 0, "the resolution generation's complete body, written once in one shot"),
    sync("GEN", "the reserved generation is durable but names nothing yet"),
    write("COMMIT.JNL", 0, SLOT_STRIDE, "the terminal record that publishes the manifest head"),
    sync("COMMIT.JNL", "its body"),
    write("COMMIT.JNL", JOURNAL_GATE_OFFSET, 512, "its gate"),
    sync("COMMIT.JNL", "the only visibility point of the release"),
];

/// The commit paths whose every cut point the crash matrix enumerates.
pub fn transcripts() -> Vec<Transcript> {
    vec![
        Transcript {
            name: "journal-append",
            section: "OBC2_Storage_Format.md §1, §6.2",
            description: "One journal record. Journal slots are the single exemption from the invalidate-first discipline, so the body write carries the zeroed gate sector itself.",
            steps: &JOURNAL_APPEND,
            outcomes: &["the projection before the record", "the projection after it"],
            note: "",
        },
        Transcript {
            name: "checkpoint-compaction",
            section: "OBC2_Storage_Format.md §6.3",
            description: "Compaction writes and gates the inactive checkpoint completely before the new epoch's first record exists.",
            steps: &COMPACTION,
            outcomes: &[
                "the old checkpoint plus its old-epoch journal suffix",
                "the new checkpoint alone, which is the same catalog at epoch E + 1",
                "the new checkpoint plus the first record of the new epoch",
            ],
            note: "PROVISIONAL, in one respect only: the body appears here as a single 65,024-byte write because that is what the reference implementation does. Section 6.3 specifies a bounded forward pass, region by region and entry by entry, staging at most one 208-byte entry plus a 512-byte sector — so a conforming writer emits many writes here, not one. The ordering around it is normative and is what the cut points test: invalidate the inactive gate, sync; write the whole body, sync; write the gate, sync; only then the first record of the new epoch. The streaming pass lands with the compaction engine and will replace this step's shape.",
        },
        Transcript {
            name: "work-seal",
            section: "OBC2_Storage_Format.md §7",
            description: "The sealed WORK slot, the one durable work fact both device profiles write.",
            steps: &WORK_SEAL,
            outcomes: &["the previous WORK slot", "the sealed WORK slot"],
            note: "",
        },
        Transcript {
            name: "arm-phase-advance",
            section: "OBC2_Storage_Format.md §10",
            description: "One handoff phase advance across the alternating ARM pair.",
            steps: &ARM_ADVANCE,
            outcomes: &["the old (sequence, phase) pair", "the strictly greater new pair"],
            note: "",
        },
        Transcript {
            name: "manifest-publication",
            section: "OBC2_Storage_Format.md §8",
            description: "The resolution generation is written and synchronized before the terminal record; only that record's gate publishes the head.",
            steps: &MANIFEST_PUBLICATION,
            outcomes: [
                "the projection before publication, with the reserved generation left as a collectable orphan",
                "the published manifest head together with its resolution generation",
            ]
            .as_slice(),
            note: "The resolution body's length is the table's, not a fixed record size, so its write appears with length zero here.",
        },
    ]
}

/// The transcript file's canonical bytes.
pub fn transcripts_json() -> String {
    let rendered: Vec<String> = transcripts()
        .iter()
        .map(|transcript| {
            let steps: Vec<String> = transcript
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    format!(
                        "        {{ \"op\": {}, \"file\": \"{}\", \"kind\": \"{}\", \"offset\": {}, \"length\": {}, \"note\": \"{}\" }}",
                        index + 1,
                        step.file,
                        step.kind,
                        step.offset,
                        step.length,
                        step.note,
                    )
                })
                .collect();
            let outcomes: Vec<String> = transcript.outcomes.iter().map(|outcome| format!("        \"{outcome}\"")).collect();
            format!(
                "    {{\n      \"name\": \"{}\",\n      \"section\": \"{}\",\n      \"description\": \"{}\",\n      \"note\": \"{}\",\n      \"stepCount\": {},\n      \"cutPoints\": {},\n      \"steps\": [\n{}\n      ],\n      \"admissibleOutcomes\": [\n{}\n      ]\n    }}",
                transcript.name,
                transcript.section,
                transcript.description,
                transcript.note,
                transcript.steps.len(),
                transcript.steps.len() * 3,
                steps.join(",\n"),
                outcomes.join(",\n"),
            )
        })
        .collect();
    format!(
        "{{\n  \"name\": \"crash-cut-transcripts\",\n  \"suite\": \"device-object-v2\",\n  \"kind\": \"storage\",\n  \"storage_format\": 1,\n  \"section\": \"OBC2_Storage_Format.md §12\",\n  \"description\": \"Every commit path's media operations in their normative order, and the states a reboot after a cut before, during or after any of them may produce. A cut is enumerated at three positions per operation: before it reaches the card, during it (a write tears the program page it was programming; a sync commits an arbitrary subset), and after it returns.\",\n  \"transcriptCount\": {},\n  \"transcripts\": [\n{}\n  ]\n}}\n",
        transcripts().len(),
        rendered.join(",\n"),
    )
}

/// Writes every storage vector to `specs/vectors/device-object-v2/storage/`.
pub fn write_all() -> std::io::Result<usize> {
    let root = dir();
    std::fs::create_dir_all(&root)?;
    let files = files();
    for file in &files {
        std::fs::write(root.join(format!("{}.json", file.name)), file.json().as_bytes())?;
    }
    std::fs::write(root.join("crash-cut-transcripts.json"), transcripts_json().as_bytes())?;
    Ok(files.len() + 1)
}

#[cfg(test)]
mod tests {
    use super::super::checkpoint;
    use super::super::error::Reason;
    use super::super::handoff::HandoffRecord;
    use super::super::init::InitRecord;
    use super::super::journal::JournalBody;
    use super::super::resolution::Resolution;
    use super::super::work::{RideRecord, WorkRecord};
    use super::*;

    fn reason_name(reason: Reason) -> &'static str {
        match reason {
            Reason::Length => "Length",
            Reason::Magic => "Magic",
            Reason::Version => "Version",
            Reason::HeaderLength => "HeaderLength",
            Reason::Reserved => "Reserved",
            Reason::BodyCrc => "BodyCrc",
            Reason::GateCrc => "GateCrc",
            Reason::Complement => "Complement",
            Reason::SlotIndex => "SlotIndex",
            Reason::Scope => "Scope",
            Reason::Sequence => "Sequence",
            Reason::Count => "Count",
            Reason::Order => "Order",
            Reason::Duplicate => "Duplicate",
            Reason::UnknownEnum => "UnknownEnum",
            Reason::Occupied => "Occupied",
            Reason::KeyBytes => "KeyBytes",
            Reason::Combination => "Combination",
            Reason::Overflow => "Overflow",
        }
    }

    /// Runs the production decoder a case is addressed to, and re-encodes on the way back where the
    /// record has a whole-slot encoder.
    fn decode(case: &Case) -> Result<(), Reason> {
        let bytes = &case.bytes;
        match case.subject {
            Subject::Gate => super::super::gate::Gate::decode(bytes, *b"O2JG", case.slot).map(|_| ()),
            // A checkpoint's round trip goes through the projection: decode the body into the
            // model, materialize it again, and require the same bytes. That is the property
            // compaction depends on — §6.3 rewrites a checkpoint from the projection its records
            // produced — so a vector that only decoded would leave the encoder unpinned.
            Subject::Checkpoint => checkpoint::validate_file(bytes, case.slot).map(|_| {
                let model = super::super::model::CatalogModel::decode_body(&bytes[..CHECKPOINT_BODY_LEN])
                    .expect("a validated checkpoint decodes");
                let mut rebuilt = std::vec![0u8; CHECKPOINT_BODY_LEN];
                model.encode_body(&mut rebuilt).expect("body");
                assert_eq!(rebuilt, bytes[..CHECKPOINT_BODY_LEN], "checkpoint re-encode differs");
                let gate = checkpoint::gate_for(&rebuilt, case.slot);
                assert_eq!(&gate.encode()[..], &bytes[CHECKPOINT_GATE_OFFSET..], "checkpoint gate re-encode differs");
            }),
            Subject::JournalSlot => JournalBody::validate_slot(bytes, case.slot).map(|record| {
                assert_eq!(&record.encode_slot()[..], &bytes[..], "journal re-encode differs");
            }),
            Subject::WorkSlot => WorkRecord::validate_slot(bytes, case.slot).map(|record| {
                assert_eq!(&record.encode_slot(case.slot)[..], &bytes[..], "work re-encode differs");
            }),
            Subject::RideSlot => RideRecord::validate_slot(bytes, case.slot).map(|record| {
                assert_eq!(&record.encode_slot(case.slot)[..], &bytes[..], "ride re-encode differs");
            }),
            Subject::ArmFile => HandoffRecord::validate_slot(bytes, case.slot).map(|record| {
                assert_eq!(&record.encode_slot(case.slot)[..], &bytes[..], "handoff re-encode differs");
            }),
            Subject::InitRecord => InitRecord::validate_slot(bytes).map(|record| {
                assert_eq!(&record.encode_slot()[..], &bytes[..], "init re-encode differs");
            }),
            // The resolution table's round trip is the same idea at a smaller size: decode it, hand
            // the entries back to the encoder, and require the same bytes.
            Subject::Resolution => Resolution::decode(bytes).map(|table| {
                let entries: std::vec::Vec<_> = table.iter().collect();
                let mut rebuilt = std::vec![0u8; super::super::resolution::MAX_BODY_LEN];
                let len = super::super::resolution::encode(&entries, &mut rebuilt).expect("entries re-encode");
                assert_eq!(&rebuilt[..len], &bytes[..], "resolution re-encode differs");
            }),
        }
        .map_err(|error| error.reason)
    }

    /// The loop that makes these vectors worth having: the codec must agree with every positive
    /// case byte for byte, and refuse every negative one with the reason the file names.
    #[test]
    fn the_production_codec_agrees_with_every_case() {
        for file in files() {
            for case in &file.cases {
                match (case.reject, decode(case)) {
                    (None, Ok(())) => {}
                    (None, Err(reason)) => {
                        panic!("{}/{}: expected acceptance, got {reason:?}", file.name, case.name)
                    }
                    (Some(expected), Err(reason)) => assert_eq!(
                        reason_name(reason),
                        expected,
                        "{}/{}: rejected for the wrong reason",
                        file.name,
                        case.name
                    ),
                    (Some(expected), Ok(())) => {
                        panic!("{}/{}: expected rejection {expected}, got acceptance", file.name, case.name)
                    }
                }
            }
        }
    }

    /// The `runs` encoding must reconstruct the case's bytes exactly, or a cross-language reader
    /// cannot use these files at all.
    #[test]
    fn runs_reconstruct_every_case() {
        for file in files() {
            for case in &file.cases {
                let mut rebuilt = zeros(case.bytes.len());
                for (offset, bytes) in runs(&case.bytes) {
                    bytes_at(&mut rebuilt, offset, &bytes);
                }
                assert_eq!(rebuilt, case.bytes, "{}/{}", file.name, case.name);
                assert_eq!(sha256(&rebuilt), sha256(&case.bytes));
            }
        }
    }

    /// §7's check value, computed by the producer's own long-form CRC.
    #[test]
    fn the_producers_crc_is_the_contract_crc() {
        assert_eq!(super::raw::crc32(b"123456789"), 0xCBF4_3926);
    }

    /// The CI guard `Device_Object_Vectors_v2.md` §7 asks for: the checked-in files are exactly what
    /// this producer emits, so an unreviewed fixture rewrite fails the build.
    #[test]
    fn checked_in_storage_vectors_match_the_producer() {
        // Drift in the other direction too: a file on disk the producer no longer emits is a
        // leftover the manifest would happily keep indexing. One assertion closes it.
        let expected: std::collections::BTreeSet<String> = files()
            .iter()
            .map(|file| format!("{}.json", file.name))
            .chain(core::iter::once("crash-cut-transcripts.json".to_string()))
            .collect();
        let mut found = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(dir()).expect("the storage directory") {
            let name = entry.expect("directory entry").file_name().to_string_lossy().into_owned();
            found.insert(name);
        }
        assert_eq!(found, expected, "the storage directory holds a file the producer does not emit, or is missing one");

        let transcripts_path = dir().join("crash-cut-transcripts.json");
        let checked_in = std::fs::read_to_string(&transcripts_path)
            .unwrap_or_else(|error| panic!("{}: {error}", transcripts_path.display()));
        assert_eq!(checked_in, transcripts_json(), "{} is stale", transcripts_path.display());

        for file in files() {
            let path = dir().join(format!("{}.json", file.name));
            let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "{}: {error}. Regenerate with `cargo test -p obc-storage regenerate_storage_vectors -- --ignored`",
                    path.display()
                )
            });
            assert_eq!(checked_in, file.json(), "{} is stale", path.display());
        }
    }

    /// The suite manifest indexes these files by name and digest; `obc-link` writes it, so this
    /// checks the two halves agree without either crate depending on the other.
    #[test]
    fn the_suite_manifest_lists_every_storage_vector() {
        let manifest_path = dir().join("../manifest.json");
        let manifest = std::fs::read_to_string(&manifest_path).expect("manifest");
        assert!(manifest.contains("storage/crash-cut-transcripts.json"));
        for file in files() {
            let entry = format!("storage/{}.json", file.name);
            assert!(manifest.contains(&entry), "{} is missing from the suite manifest", entry);
            let digest = sha256(file.json().as_bytes());
            assert!(manifest.contains(&digest), "{}'s digest is stale in the suite manifest", entry);
        }
    }

    #[test]
    #[ignore = "regenerates the checked-in storage vectors"]
    fn regenerate_storage_vectors() {
        let count = write_all().expect("write");
        std::println!("wrote {count} storage vector files to {}", dir().display());
    }
}
