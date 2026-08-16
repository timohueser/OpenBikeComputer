//! The immutable generation writer: begin, append, seal (`OBC2_Storage_Format.md` §3, §7).
//!
//! A generation file "is exactly the canonical payload bytes and contains no OBC2 wrapper" (§3). It
//! is created under a unique `GenerationId`, never modified after seal, and never made visible by
//! existing — only a catalog commit publishes it. This module owns what §1 gives the kernel over
//! those bytes and nothing else: **the byte count, the rolling CRC, the sync and ordering
//! discipline, and the one durable work fact seal writes.** It parses no payload and knows no
//! domain.
//!
//! ## Restart-only, and what that removes
//!
//! The initial device ships the restart-only upload profile (§7, and the DOS2 owner decision of
//! 2026-08-16). §7: "A device that advertises no resumable kind never records durable upload
//! progress: its claims stand alone, every readmission truncates the claimed generation and streams
//! from offset zero, and recovery classifies a claimed, unsealed generation as restartable work at
//! offset zero." So there are no durable checkpoints here, no resume record, no offset to
//! acknowledge — and consequently no streaming `WORK` slot is ever written.
//!
//! **Seal still writes its sealed `WORK` slot**, which §7 calls "the one durable work fact both
//! profiles share", and the `WORK` file's layout and preallocation stay part of the frozen format so
//! a card moves between profiles without conversion.
//!
//! ## The restart durability point
//!
//! §7 states a rule about rewinding to zero that is easy to satisfy accidentally and expensive to
//! get wrong: "Before a single byte is accepted at offset zero, `CardStore` writes and synchronizes
//! a WORK slot … Only after that slot's gate is durable may the payload file be truncated or
//! rewritten." The fault it forbids is a payload overwritten while a `WORK` slot still records the
//! old offset and prefix CRC, which makes recovery re-read a prefix that no longer matches and
//! terminally abort a healthy upload.
//!
//! Under the restart-only profile that rule is satisfied **vacuously**, because no slot recording a
//! streaming offset ever exists — and this module is written so that the vacuity is a fact rather
//! than an assumption. [`GenerationWriter::restart`] truncates and synchronizes *before* it will
//! accept a byte, and it does so by invalidating the capability every earlier append held: a caller
//! that kept one cannot write at a stale offset even by mistake. That is the same ordering the
//! resumable profile will need when it arrives, minus the slot.
//!
//! ## Where the seal's own crash safety comes from
//!
//! Nothing in this module protects the seal itself, and that is deliberate. `seal` performs six
//! media operations after the payload sync, any of which can be cut, and the writer's own state is
//! gone at the next boot — so the safety cannot live here. It lives in [`work::recover_work`], and
//! specifically in §7's reachability filter: a slot "recording an offset the payload cannot reach is
//! skipped as if invalid", and a qualifying slot is resumed only after its finalized prefix CRC is
//! proved against the bytes the payload now holds.
//!
//! That covers the two orderings a cut can leave. A slot written over a payload whose sync did not
//! make its length durable records an offset the file cannot reach and is skipped, which is why the
//! payload sync comes first and why its observed length is checked before the record is built. A
//! slot torn part-way through its own stride fails its gate and is not a record at all. Either way
//! recovery reaches "restartable work at offset zero" rather than a sealed record the card does not
//! back — and the crash matrix asserts exactly that, recomputing the prefix CRC from the durable
//! payload at every cut point rather than trusting the slot.
//!
//! ## The seam
//!
//! [`GenerationMedia`] is the media this writer needs, in the terms §7 uses: a payload file that
//! grows, and a fixed-length two-slot `WORK` file. It is the §13.1 adapter's shape — the `WORK`
//! file is a gated fixed file that [`Adapter`](super::adapter::Adapter) owns outright, and the
//! payload is an ordinary growable file §13.1 does not constrain beyond its seek bound and its write
//! completeness. Naming the seam rather than taking the adapter directly is what lets the crash
//! matrix drive this against the faulting harness; production wiring is #1359's.

use obc_link::ids::{DraftPartRef, GenerationId, OperationId, StoreId};

use super::gate::INVALIDATED;
use super::limits::{GATE_LEN, MAX_GENERATION_LEN, SLOT_STRIDE, SMALL_GATE_OFFSET, WORK_SLOTS};
use super::work::{Subject, WorkRecord, WorkState};

/// The media one generation transaction addresses (§7, §13.1).
pub trait GenerationMedia {
    /// What a media operation can fail with.
    type Error;

    /// Creates this generation's `GEN` and `WORK` shard directories if they are not already there.
    ///
    /// The lazy-shard obligation §11 and §12 place on admission, in the one place a generation's
    /// files are about to be created. It is `make_dir` on a possibly already-present directory,
    /// which §12 makes "not an error", so a second call costs nothing — about 140 ms the first time
    /// a shard is used and nothing afterwards. [`mount::shard_to_create`] names which shard.
    ///
    /// **The implementation is #1359's.** The writer calls it before the first byte of a transaction
    /// so the obligation cannot be forgotten at the seam that needs it, but nothing in this slice
    /// creates a directory: the crash harness satisfies it by recording the call, and the board's
    /// implementation arrives with the store that owns the FAT handles.
    ///
    /// [`mount::shard_to_create`]: super::mount::shard_to_create
    fn ensure_shards(&mut self, generation: GenerationId) -> Result<(), Self::Error>;

    /// The payload file's recorded length, as observed after a sync (§7's `observed payload file
    /// length`).
    fn payload_length(&mut self) -> Result<u64, Self::Error>;

    /// Writes payload bytes at `offset`, extending the file when the write runs past its end.
    ///
    /// §13.1's write completeness is the implementation's: "a short write is an error, never a
    /// success", so this returns `Ok` only when every byte was accepted.
    fn write_payload(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Persists the payload bytes and the length the directory entry records for them.
    fn sync_payload(&mut self) -> Result<(), Self::Error>;

    /// Truncates the payload to zero length.
    fn truncate_payload(&mut self) -> Result<(), Self::Error>;

    /// Writes into this generation's fixed-length 32,768-byte `WORK` file.
    fn write_work(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Persists the `WORK` file. §13.1's clean flush: the file's length never changes, so this
    /// writes no directory entry and no FSInfo.
    fn sync_work(&mut self) -> Result<(), Self::Error>;
}

/// What a generation transaction is for: the identity `BeginWork` bound and the payload it declared.
///
/// Every field here is fixed at claim and immutable afterwards. §7's `WORK` body carries all of
/// them, which is why they are one value rather than seal arguments: a seal that could restate the
/// declared length would not be sealing the thing that was claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intent {
    /// The store.
    pub store: StoreId,
    /// The child for a draft part, the parent for its manifest, or the ordinary operation.
    pub operation: OperationId,
    /// Its canonical-intent digest.
    pub intent: [u8; 32],
    /// The parent claim for a draft child; inactive zero otherwise.
    pub parent: OperationId,
    /// The generation these bytes are being written as.
    pub generation: GenerationId,
    /// The declared payload length.
    pub declared_length: u64,
    /// The declared payload CRC-32.
    pub declared_crc: u32,
    /// `ObjectKind` or `DraftPartKind`.
    pub subject_kind: u16,
    /// Which of those two the kind is.
    pub subject: Subject,
    /// The draft part key, or zero.
    pub part_key: u64,
}

/// The opaque, generation-specific capability a transaction is driven through (§ "Transaction API"
/// of #1354).
///
/// Its fields are private: only this module can compare one against a writer, so "a stale or wrong
/// capability cannot append, seal, publish or abort another transaction" is a property of the type
/// rather than of every call site. The generation makes it specific to one transaction; the nonce
/// makes it specific to one *tenancy* of that transaction, which is what a restart ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    generation: GenerationId,
    nonce: u32,
}

impl Capability {
    /// The generation this capability names. The one field a caller legitimately reads.
    pub fn generation(&self) -> GenerationId {
        self.generation
    }
}

/// Where a transaction is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterState {
    /// Accepting payload bytes.
    Streaming,
    /// Sealed: exact length and whole-object CRC proved, the sealed `WORK` slot durable. §7: "A
    /// sealed generation is immutable."
    Sealed,
    /// Abandoned. The payload and `WORK` files become garbage-collection input once the operation's
    /// terminal record is durable.
    Aborted,
}

/// Why a generation operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError<E> {
    /// The media failed.
    Media(E),
    /// The capability is not this transaction's, or is from a tenancy a restart ended.
    Capability,
    /// The transaction is sealed or aborted, so it accepts nothing further.
    NotStreaming,
    /// The write would take the payload past its declared length. §7: "The durable offset cannot
    /// exceed the declared length", and a generation is what it declared or it is nothing.
    Overrun {
        /// The declared length.
        declared: u64,
        /// Where the write would have ended.
        would_reach: u64,
    },
    /// Seal found a byte count other than the declared one.
    Length {
        /// The declared length.
        declared: u64,
        /// What was actually written.
        written: u64,
    },
    /// Seal found a whole-object CRC other than the declared one.
    Crc {
        /// The declared CRC-32.
        declared: u32,
        /// What the accepted bytes actually hash to.
        computed: u32,
    },
    /// The payload's observed length cannot back the offset the seal would record.
    ///
    /// §13.1's seek bound makes this unrecoverable rather than merely stale: "the adapter cannot
    /// seek beyond a file's recorded length, so a durable offset above the observed length is not
    /// merely stale, it is unreachable".
    Unreachable {
        /// The offset the record would have carried.
        durable: u64,
        /// What the payload's directory entry says.
        observed: u64,
    },
    /// The declared length is above the §2 single-generation limit.
    TooLarge {
        /// The declared length.
        declared: u64,
    },
}

/// One immutable generation being written.
///
/// The writer holds no buffer: bytes go straight to the payload file as they arrive, and what stays
/// resident is the count, the rolling CRC and the state. §13's arena is not asked for a staging
/// buffer here, and seal borrows the caller's stride scratch rather than owning 16 KiB.
pub struct GenerationWriter {
    intent: Intent,
    nonce: u32,
    written: u64,
    crc: obc_crc::Crc32,
    state: WriterState,
    /// Whether this transaction has already made its shard directories exist. A shard, once
    /// created, stays; a restart does not undo it.
    shards_ready: bool,
    /// The `WORK` slot the sealed record will occupy. The restart-only profile writes no streaming
    /// slot, so the pair is empty and seal takes slot zero; the alternating discipline is still the
    /// one §7 states, and the resumable profile will simply arrive with a nonzero value here.
    work_slot: usize,
    work_sequence: u32,
}

impl GenerationWriter {
    /// Opens a transaction over an already-claimed generation.
    ///
    /// This writes nothing. §7: "`BeginWork` reserves the next GenerationId and the preflighted
    /// logical resources in the catalog journal before either physical file is created" — the claim
    /// is a journal record the caller appends, and this is what happens afterwards.
    pub fn begin<E>(intent: Intent) -> Result<(Self, Capability), WriteError<E>> {
        if intent.declared_length > MAX_GENERATION_LEN {
            return Err(WriteError::TooLarge { declared: intent.declared_length });
        }
        let writer = GenerationWriter {
            intent,
            nonce: 1,
            written: 0,
            crc: obc_crc::Crc32::new(),
            state: WriterState::Streaming,
            shards_ready: false,
            work_slot: 0,
            work_sequence: 0,
        };
        let capability = writer.capability();
        Ok((writer, capability))
    }

    /// The capability of the current tenancy.
    pub fn capability(&self) -> Capability {
        Capability { generation: self.intent.generation, nonce: self.nonce }
    }

    /// The generation being written.
    pub fn generation(&self) -> GenerationId {
        self.intent.generation
    }

    /// How many bytes have been accepted.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Where the transaction is.
    pub fn state(&self) -> WriterState {
        self.state
    }

    /// Accepts payload bytes at the current offset, returning the new offset.
    ///
    /// No sync happens here and none is owed: the restart-only profile acknowledges no offset, so
    /// there is nothing a durable point would make true. The bytes become durable at seal, which is
    /// the only offset this profile ever acknowledges.
    pub fn append<M: GenerationMedia>(
        &mut self,
        capability: Capability,
        media: &mut M,
        bytes: &[u8],
    ) -> Result<u64, WriteError<M::Error>> {
        self.authorize(capability)?;
        let would_reach = self.written.saturating_add(bytes.len() as u64);
        if would_reach > self.intent.declared_length {
            return Err(WriteError::Overrun { declared: self.intent.declared_length, would_reach });
        }
        // §12's lazy shards: the leaf's directory has to exist before the leaf does, and this is the
        // last moment before the payload file is addressed. Idempotent, so the flag is an
        // optimization rather than a correctness condition.
        if !self.shards_ready {
            media.ensure_shards(self.intent.generation).map_err(WriteError::Media)?;
            self.shards_ready = true;
        }
        media.write_payload(self.written, bytes).map_err(WriteError::Media)?;
        self.crc.update(bytes);
        self.written = would_reach;
        Ok(self.written)
    }

    /// Rewinds this generation to offset zero and issues a fresh capability (§7's restart
    /// durability point).
    ///
    /// The order is the rule: the payload is truncated and **synchronized** before a byte is
    /// accepted at offset zero again, and every capability of the previous tenancy dies at the same
    /// moment. A caller holding one gets [`WriteError::Capability`] rather than an append at a stale
    /// offset.
    ///
    /// This is the whole of readmission under the restart-only profile. No `WORK` slot is written
    /// because none records a streaming offset, which is exactly why §7 calls the durability point
    /// satisfied vacuously here — the stale-offset fault the rule closes has no state to arise from.
    pub fn restart<M: GenerationMedia>(
        &mut self,
        capability: Capability,
        media: &mut M,
    ) -> Result<Capability, WriteError<M::Error>> {
        self.authorize(capability)?;
        // The old tenancy ends **before** the media is touched, not after it succeeds. From the
        // first byte of the truncation onwards the payload no longer matches the offset the old
        // capability was counting from, so a truncation that fails half-way must not leave that
        // capability able to append: the writer would carry on at an offset the file no longer has.
        // Ending it up front makes the failure path the same as the success path — the caller
        // retries the restart, or aborts — instead of a state where a stale offset is still live.
        self.nonce = self.nonce.wrapping_add(1);
        self.written = 0;
        self.crc = obc_crc::Crc32::new();
        media.truncate_payload().map_err(WriteError::Media)?;
        media.sync_payload().map_err(WriteError::Media)?;
        // Only now, with both returned, is the rewind durable and the fresh capability usable.
        Ok(self.capability())
    }

    /// Seals the generation: proves the declared length and CRC, makes the payload durable, and
    /// writes the sealed `WORK` slot (§7).
    ///
    /// `scratch` is the 16,384-byte stride buffer the slot is assembled in; the caller owns it
    /// because a 16 KiB stack temporary is not something the board's task stacks can spend. `part_ref`
    /// is the opaque reference §5.3 mints at the seal of a draft child, and is
    /// [`DraftPartRef::ZERO`] for every other subject.
    ///
    /// The returned record is the durable work fact. Domain validation runs after it and never
    /// before: §7 puts the payload sync and the sealed slot ahead of "only then allows domain
    /// validation", because a validator that read an unsynced payload would be validating a file the
    /// card may not hold.
    pub fn seal<M: GenerationMedia>(
        &mut self,
        capability: Capability,
        media: &mut M,
        scratch: &mut [u8; SLOT_STRIDE],
        part_ref: DraftPartRef,
    ) -> Result<WorkRecord, WriteError<M::Error>> {
        self.authorize(capability)?;
        if self.written != self.intent.declared_length {
            return Err(WriteError::Length { declared: self.intent.declared_length, written: self.written });
        }
        // A generation of zero declared bytes never appends, so the shard the `WORK` leaf lands in
        // would never have been created — and the seal would write a slot into a directory that
        // does not exist. Nothing in §5, §7 or the wire contract forbids a zero-length payload
        // (`Device_Object_Protocol_v3.md` §7's "nonempty payload" rule is about a Data *frame*, not
        // about the object), so the seal ensures the shard rather than `begin` refusing the length.
        // Guarded, so an ordinary upload that already appended does not ask twice.
        if !self.shards_ready {
            media.ensure_shards(self.intent.generation).map_err(WriteError::Media)?;
            self.shards_ready = true;
        }
        let computed = self.crc.clone().finalize();
        if computed != self.intent.declared_crc {
            return Err(WriteError::Crc { declared: self.intent.declared_crc, computed });
        }

        // §7 step 1 and 2 of a checkpoint, which a seal is the last of: the payload bytes are
        // synchronized and its length observed, before anything names them.
        media.sync_payload().map_err(WriteError::Media)?;
        let observed = media.payload_length().map_err(WriteError::Media)?;
        if observed < self.written {
            return Err(WriteError::Unreachable { durable: self.written, observed });
        }

        let record = WorkRecord {
            store: self.intent.store,
            operation: self.intent.operation,
            intent: self.intent.intent,
            parent: self.intent.parent,
            part_ref,
            generation: self.intent.generation,
            declared_length: self.intent.declared_length,
            declared_crc: self.intent.declared_crc,
            state: WorkState::Sealed,
            flags: 0,
            durable_offset: self.written,
            prefix_crc: computed,
            sequence: self.work_sequence,
            progress_counter: 0,
            subject_kind: self.intent.subject_kind,
            subject: self.intent.subject,
            part_key: self.intent.part_key,
            // §13.1 bounds a file length at `0xFFFF_FFFF`, which is also the single-generation
            // limit `begin` refused above, so this cast cannot lose a bit that mattered.
            observed_length: observed.min(MAX_GENERATION_LEN) as u32,
        };
        write_gated_work_slot(media, self.work_slot, &record, scratch)?;

        self.state = WriterState::Sealed;
        // A sealed generation is immutable, and the capability that sealed it is spent: no later
        // call may append to, restart or re-seal these bytes.
        self.nonce = self.nonce.wrapping_add(1);
        Ok(record)
    }

    /// Abandons the transaction.
    ///
    /// Nothing on the card changes. §6.2 makes the durable act a terminal `Aborted` journal record,
    /// and only after its gate "may its WORK/payload become collectible" — so an abort here is the
    /// RAM half, and the files it leaves behind are ordinary garbage-collection input.
    pub fn abort<E>(&mut self, capability: Capability) -> Result<(), WriteError<E>> {
        self.authorize(capability)?;
        self.state = WriterState::Aborted;
        self.nonce = self.nonce.wrapping_add(1);
        Ok(())
    }

    fn authorize<E>(&self, capability: Capability) -> Result<(), WriteError<E>> {
        if self.state != WriterState::Streaming {
            return Err(WriteError::NotStreaming);
        }
        if capability != self.capability() {
            return Err(WriteError::Capability);
        }
        Ok(())
    }
}

/// Writes one `WORK` slot in §1's order: invalidate this slot's gate, write its body across the
/// whole stride, write its gate — each followed by its own sync.
///
/// The body write covers the stride with the gate sector zeroed, for the reason §1 gives: a slot a
/// previous cut tore holds arbitrary bytes across its whole program page and a reader rejects a
/// nonzero pad, so rewriting only the body bytes could never return the slot to validity.
fn write_gated_work_slot<M: GenerationMedia>(
    media: &mut M,
    slot: usize,
    record: &WorkRecord,
    scratch: &mut [u8; SLOT_STRIDE],
) -> Result<(), WriteError<M::Error>> {
    debug_assert!(slot < WORK_SLOTS, "a WORK file has two slots");
    let base = slot * SLOT_STRIDE;
    // `encode_slot_into` only fails on a wrongly sized buffer, and this one is a stride by type.
    record.encode_slot_into(scratch.as_mut_slice(), slot as u16).expect("a stride-sized scratch");
    let mut gate = [0u8; GATE_LEN];
    gate.copy_from_slice(&scratch[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + GATE_LEN]);
    scratch[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + GATE_LEN].fill(0);

    media.write_work(base + SMALL_GATE_OFFSET, &INVALIDATED).map_err(WriteError::Media)?;
    media.sync_work().map_err(WriteError::Media)?;
    media.write_work(base, scratch.as_slice()).map_err(WriteError::Media)?;
    media.sync_work().map_err(WriteError::Media)?;
    media.write_work(base + SMALL_GATE_OFFSET, &gate).map_err(WriteError::Media)?;
    media.sync_work().map_err(WriteError::Media)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    use super::super::samples;

    /// A memoryless medium: the payload is a growable buffer, the `WORK` file is 32,768 fixed bytes,
    /// and every operation is recorded so the ordering can be asserted.
    #[derive(Debug, Default)]
    struct Fake {
        payload: Vec<u8>,
        durable_payload_len: u64,
        pending: Vec<(u64, Vec<u8>)>,
        work: Vec<u8>,
        log: Vec<&'static str>,
        shards: Vec<super::super::names::ShardName>,
        fail_at: Option<usize>,
        ops: usize,
    }

    impl Fake {
        fn new() -> Self {
            Fake { work: vec![0u8; super::super::limits::WORK_FILE_LEN], ..Fake::default() }
        }

        fn step(&mut self, what: &'static str) -> Result<(), &'static str> {
            self.ops += 1;
            self.log.push(what);
            if self.fail_at == Some(self.ops) {
                return Err("injected");
            }
            Ok(())
        }
    }

    impl GenerationMedia for Fake {
        type Error = &'static str;

        fn ensure_shards(&mut self, generation: GenerationId) -> Result<(), &'static str> {
            self.step("ensure shards")?;
            self.shards.push(super::super::names::LeafName::of(generation).shard);
            Ok(())
        }

        fn payload_length(&mut self) -> Result<u64, &'static str> {
            Ok(self.durable_payload_len)
        }

        fn write_payload(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
            self.step("payload write")?;
            self.pending.push((offset, bytes.to_vec()));
            Ok(())
        }

        fn sync_payload(&mut self) -> Result<(), &'static str> {
            self.step("payload sync")?;
            for (offset, bytes) in core::mem::take(&mut self.pending) {
                let end = offset as usize + bytes.len();
                if self.payload.len() < end {
                    self.payload.resize(end, 0);
                }
                self.payload[offset as usize..end].copy_from_slice(&bytes);
            }
            self.durable_payload_len = self.payload.len() as u64;
            Ok(())
        }

        fn truncate_payload(&mut self) -> Result<(), &'static str> {
            self.step("payload truncate")?;
            self.payload.clear();
            self.pending.clear();
            self.durable_payload_len = 0;
            Ok(())
        }

        fn write_work(&mut self, offset: usize, bytes: &[u8]) -> Result<(), &'static str> {
            self.step("work write")?;
            self.work[offset..offset + bytes.len()].copy_from_slice(bytes);
            Ok(())
        }

        fn sync_work(&mut self) -> Result<(), &'static str> {
            self.step("work sync")
        }
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index * 31 + 7) as u8).collect()
    }

    fn intent(bytes: &[u8]) -> Intent {
        Intent {
            store: samples::STORE,
            operation: OperationId::new(samples::OP_A),
            intent: samples::INTENT,
            parent: OperationId::ZERO,
            generation: GenerationId::new(42),
            declared_length: bytes.len() as u64,
            declared_crc: obc_crc::crc32(bytes),
            subject_kind: 1,
            subject: Subject::LogicalObject,
            part_key: 0,
        }
    }

    fn stride() -> std::boxed::Box<[u8; SLOT_STRIDE]> {
        std::boxed::Box::new([0u8; SLOT_STRIDE])
    }

    #[test]
    fn a_generation_streams_and_seals_into_a_sealed_work_slot() {
        let bytes = payload(3_000);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();

        for chunk in bytes.chunks(512) {
            writer.append(capability, &mut media, chunk).unwrap();
        }
        assert_eq!(writer.written(), 3_000);

        let mut scratch = stride();
        let record = writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
        assert_eq!(writer.state(), WriterState::Sealed);
        assert_eq!(record.state, WorkState::Sealed);
        assert_eq!(record.durable_offset, 3_000);
        assert_eq!(record.prefix_crc, obc_crc::crc32(&bytes));
        assert_eq!(record.declared_crc, record.prefix_crc, "a sealed record's prefix is the whole object");
        assert_eq!(record.observed_length, 3_000);

        // The slot on the card is the record, at slot zero, and it validates whole.
        let slot: &[u8; SLOT_STRIDE] = media.work[..SLOT_STRIDE].try_into().unwrap();
        assert_eq!(WorkRecord::validate_slot(slot, 0).unwrap(), record);
        assert_eq!(media.payload, bytes, "the payload on the card is not what was appended");
    }

    /// §7's ordering, as the log: the payload is durable before anything names it, the slot's gate
    /// is invalidated before its body is rewritten, and the gate is last.
    #[test]
    fn seal_performs_the_media_operations_in_section_7s_order() {
        let bytes = payload(1_024);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        writer.append(capability, &mut media, &bytes).unwrap();
        let mut scratch = stride();
        writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();

        assert_eq!(
            media.log,
            std::vec![
                "ensure shards", // §12's lazy leaf directory, before the leaf can exist
                "payload write",
                "payload sync",
                "work write", // invalidate the gate of the slot about to be used
                "work sync",
                "work write", // the sealed body across the whole stride, gate sector zeroed
                "work sync",
                "work write", // the O2WG gate
                "work sync",
            ],
        );
        // The shard is the one §3 maps this generation to, and it is asked for exactly once.
        assert_eq!(media.shards, std::vec![super::super::names::LeafName::of(GenerationId::new(42)).shard]);
    }

    /// A generation of zero declared bytes never appends, so `seal` is the only place its shard can
    /// be created — and without that call the sealed `WORK` slot would be written into a directory
    /// that does not exist.
    ///
    /// Zero is a legal declared length: §5, §7 and §2's `0xFFFF_FFFF` ceiling all admit it, and the
    /// wire contract's "nonempty payload" rule is about a Data *frame* rather than about the object,
    /// so a zero-length upload simply sends none. Refusing it at `begin` would invent a limit the
    /// format does not have, which is why the fix is the seal-side call and not a new rejection.
    #[test]
    fn a_zero_length_generation_ensures_its_shard_at_seal() {
        let empty: Vec<u8> = Vec::new();
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&empty)).unwrap();
        let mut media = Fake::new();
        let mut scratch = stride();

        // Nothing is appended, so nothing has touched the medium at all.
        assert!(media.log.is_empty());
        let record = writer.seal(capability, &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();

        assert_eq!(media.log[0], "ensure shards", "the sealed slot was written into an uncreated shard");
        assert_eq!(media.shards, std::vec![super::super::names::LeafName::of(GenerationId::new(42)).shard]);
        assert_eq!(record.declared_length, 0);
        assert_eq!(record.durable_offset, 0);
        assert_eq!(record.prefix_crc, obc_crc::crc32(&[]), "the empty prefix's CRC is the empty CRC");
        assert_eq!(record.observed_length, 0);
        let slot: &[u8; SLOT_STRIDE] = media.work[..SLOT_STRIDE].try_into().unwrap();
        assert_eq!(WorkRecord::validate_slot(slot, 0).unwrap(), record);
    }

    /// An ordinary upload asks once, not twice: the append made the shard and the seal's guard sees
    /// it already done.
    #[test]
    fn a_streamed_generation_does_not_ensure_its_shard_twice() {
        let bytes = payload(512);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        let mut scratch = stride();
        writer.append(capability, &mut media, &bytes).unwrap();
        writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
        assert_eq!(media.shards.len(), 1);
        assert_eq!(media.log.iter().filter(|step| **step == "ensure shards").count(), 1);
    }

    /// The shard is created once per transaction, before the payload exists, and a restart does not
    /// ask again — a directory, once made, stays.
    #[test]
    fn the_lazy_shard_is_ensured_once_before_the_payload_is_addressed() {
        let bytes = payload(1_024);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        assert!(media.shards.is_empty(), "begin touched the medium");

        for chunk in bytes.chunks(128) {
            writer.append(writer.capability(), &mut media, chunk).unwrap();
        }
        assert_eq!(media.shards.len(), 1, "the shard was asked for more than once");
        assert_eq!(media.log[0], "ensure shards", "the payload was addressed before its directory existed");

        let fresh = writer.restart(capability, &mut media).unwrap();
        writer.append(fresh, &mut media, &bytes).unwrap();
        assert_eq!(media.shards.len(), 1, "a restart asked for a directory that already exists");
    }

    /// The one durable work fact both profiles share is the *only* slot this profile writes: no
    /// streaming slot exists at any point of an upload.
    #[test]
    fn the_restart_only_profile_writes_no_streaming_work_slot() {
        let bytes = payload(4_096);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        for chunk in bytes.chunks(256) {
            writer.append(writer.capability(), &mut media, chunk).unwrap();
        }
        let _ = capability;
        // Sixteen appends and not one touched the WORK file.
        assert!(
            media.log.iter().all(|step| step.starts_with("payload") || *step == "ensure shards"),
            "{:?}",
            media.log,
        );
        assert!(media.work.iter().all(|&byte| byte == 0), "a streaming slot was written");

        let mut scratch = stride();
        writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
        // And exactly one slot exists afterwards: slot 1 is still untouched zeros.
        let second: &[u8; SLOT_STRIDE] = media.work[SLOT_STRIDE..].try_into().unwrap();
        assert!(WorkRecord::validate_slot(second, 1).is_err());
    }

    /// §7's restart durability point: truncate and sync happen before a byte is accepted at zero,
    /// and the capability every earlier append held is dead the moment they return.
    #[test]
    fn restart_truncates_and_synchronizes_before_it_accepts_a_byte() {
        let bytes = payload(2_048);
        let (mut writer, first) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        writer.append(first, &mut media, &bytes[..1_024]).unwrap();
        media.sync_payload().unwrap();
        assert_eq!(media.durable_payload_len, 1_024);

        media.log.clear();
        let second = writer.restart(first, &mut media).unwrap();
        assert_eq!(media.log, std::vec!["payload truncate", "payload sync"]);
        assert_eq!(media.durable_payload_len, 0);
        assert_eq!(writer.written(), 0);

        // The old capability cannot write at the stale offset — which is the fault the rule exists
        // to forbid, made structurally impossible rather than merely documented.
        assert_eq!(writer.append(first, &mut media, &bytes), Err(WriteError::Capability));
        assert_ne!(first, second);

        // And the fresh one streams the whole object from zero and seals.
        writer.append(second, &mut media, &bytes).unwrap();
        let mut scratch = stride();
        let record = writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
        assert_eq!(record.durable_offset, 2_048);
        assert_eq!(record.prefix_crc, obc_crc::crc32(&bytes));
        assert_eq!(media.payload, bytes);
    }

    /// A restart that fails half-way must still have ended the old tenancy.
    ///
    /// The truncation is the moment the payload stops matching the offset the old capability was
    /// counting from, and a failure gives no evidence about how much of it landed. If the old
    /// capability survived a failed restart it could append at that stale offset over a file the
    /// card may already have shortened — so the tenancy ends before the medium is touched, and the
    /// only way forward is the fresh capability.
    #[test]
    fn a_failed_restart_still_ends_the_old_tenancy() {
        let bytes = payload(2_048);
        for failing_step in [1usize, 2] {
            let (mut writer, first) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
            let mut media = Fake::new();
            writer.append(first, &mut media, &bytes[..1_024]).unwrap();
            media.sync_payload().unwrap();

            // Step 1 is the truncation, step 2 the sync that makes it durable.
            media.fail_at = Some(media.ops + failing_step);
            assert_eq!(writer.restart(first, &mut media), Err(WriteError::Media("injected")));

            // The old capability is dead however far the truncation got.
            assert_eq!(
                writer.append(first, &mut media, &bytes),
                Err(WriteError::Capability),
                "step {failing_step}: a failed restart left the stale capability able to append",
            );
            assert_eq!(writer.written(), 0, "step {failing_step}: the writer still believes an old offset");

            // And the fresh one restarts cleanly, which is the whole recovery path.
            media.fail_at = None;
            let second = writer.restart(writer.capability(), &mut media).unwrap();
            writer.append(second, &mut media, &bytes).unwrap();
            let mut scratch = stride();
            let record = writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
            assert_eq!(record.durable_offset, bytes.len() as u64);
            assert_eq!(media.payload, bytes, "step {failing_step}: the payload is not the restreamed object");
        }
    }

    /// A capability from another generation, or from a spent tenancy, drives nothing.
    #[test]
    fn a_stale_or_foreign_capability_cannot_append_seal_or_abort() {
        let bytes = payload(512);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut other_intent = intent(&bytes);
        other_intent.generation = GenerationId::new(43);
        let (_other, foreign) = GenerationWriter::begin::<&str>(other_intent).unwrap();
        let mut media = Fake::new();
        let mut scratch = stride();

        assert_eq!(writer.append(foreign, &mut media, &bytes), Err(WriteError::Capability));
        assert_eq!(writer.seal(foreign, &mut media, &mut scratch, DraftPartRef::ZERO), Err(WriteError::Capability));
        assert_eq!(writer.abort::<&str>(foreign), Err(WriteError::Capability));
        assert!(media.log.is_empty(), "a refused capability reached the medium");

        // A sealed transaction accepts nothing further, whatever capability is offered.
        writer.append(capability, &mut media, &bytes).unwrap();
        writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
        assert_eq!(writer.append(capability, &mut media, &bytes), Err(WriteError::NotStreaming));
        assert_eq!(
            writer.seal(capability, &mut media, &mut scratch, DraftPartRef::ZERO),
            Err(WriteError::NotStreaming),
            "§7: a sealed generation is immutable",
        );
    }

    /// The three things seal proves before it makes anything durable.
    #[test]
    fn seal_refuses_a_wrong_length_a_wrong_crc_and_an_unreachable_offset() {
        let bytes = payload(1_000);
        let mut scratch = stride();

        // Short.
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        writer.append(capability, &mut media, &bytes[..900]).unwrap();
        assert_eq!(
            writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO),
            Err(WriteError::Length { declared: 1_000, written: 900 }),
        );

        // Right length, wrong bytes.
        let mut wrong = intent(&bytes);
        wrong.declared_crc ^= 0xFFFF;
        let (mut writer, capability) = GenerationWriter::begin::<&str>(wrong).unwrap();
        let mut media = Fake::new();
        writer.append(capability, &mut media, &bytes).unwrap();
        assert_eq!(
            writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO),
            Err(WriteError::Crc { declared: wrong.declared_crc, computed: obc_crc::crc32(&bytes) }),
        );

        // A payload whose recorded length a cut left short: §13.1's seek bound makes the offset
        // unreachable rather than merely stale.
        struct ShortLength(Fake);
        impl GenerationMedia for ShortLength {
            type Error = &'static str;
            fn ensure_shards(&mut self, generation: GenerationId) -> Result<(), &'static str> {
                self.0.ensure_shards(generation)
            }
            fn payload_length(&mut self) -> Result<u64, &'static str> {
                Ok(self.0.durable_payload_len.saturating_sub(1))
            }
            fn write_payload(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
                self.0.write_payload(offset, bytes)
            }
            fn sync_payload(&mut self) -> Result<(), &'static str> {
                self.0.sync_payload()
            }
            fn truncate_payload(&mut self) -> Result<(), &'static str> {
                self.0.truncate_payload()
            }
            fn write_work(&mut self, offset: usize, bytes: &[u8]) -> Result<(), &'static str> {
                self.0.write_work(offset, bytes)
            }
            fn sync_work(&mut self) -> Result<(), &'static str> {
                self.0.sync_work()
            }
        }
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = ShortLength(Fake::new());
        writer.append(capability, &mut media, &bytes).unwrap();
        assert_eq!(
            writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO),
            Err(WriteError::Unreachable { durable: 1_000, observed: 999 }),
        );
    }

    /// A refused seal leaves the transaction streaming, so the payload can be corrected and sealed
    /// again — the length and CRC checks are a proof, not a state transition.
    #[test]
    fn a_refused_seal_leaves_the_transaction_streaming() {
        let bytes = payload(1_000);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        let mut scratch = stride();
        writer.append(capability, &mut media, &bytes[..600]).unwrap();
        assert!(writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).is_err());
        assert_eq!(writer.state(), WriterState::Streaming);
        assert_eq!(writer.capability(), capability, "a refused seal must not spend the capability");
        writer.append(capability, &mut media, &bytes[600..]).unwrap();
        assert!(writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).is_ok());
    }

    /// A generation is what it declared: a byte past the declared length is refused before it
    /// reaches the medium.
    #[test]
    fn an_append_past_the_declared_length_is_refused_before_the_write() {
        let bytes = payload(100);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        writer.append(capability, &mut media, &bytes).unwrap();
        assert_eq!(
            writer.append(capability, &mut media, &[0u8; 1]),
            Err(WriteError::Overrun { declared: 100, would_reach: 101 }),
        );
        // The shard call and the one accepted write; the refused append added neither.
        assert_eq!(media.log, std::vec!["ensure shards", "payload write"], "the refused append reached the medium");
        assert_eq!(writer.written(), 100);
    }

    /// §2's single-generation limit is a §11 admission fact, and `begin` refuses rather than
    /// discovering it at the first unreachable offset.
    #[test]
    fn a_declared_length_above_the_format_limit_is_refused_at_begin() {
        let mut too_large = intent(&[]);
        too_large.declared_length = MAX_GENERATION_LEN + 1;
        assert_eq!(
            GenerationWriter::begin::<&str>(too_large).map(|_| ()),
            Err(WriteError::TooLarge { declared: MAX_GENERATION_LEN + 1 }),
        );
    }

    /// A media failure at **every** step of the seal is reported as itself and leaves the
    /// transaction streaming — recovery, not the writer, decides what the half-written slot means.
    ///
    /// The seal's seven counted operations are enumerated relative to where the append left the
    /// medium, so the loop cannot silently stop covering them when the sequence gains or loses one:
    /// the count is asserted first.
    #[test]
    fn a_media_failure_anywhere_in_the_seal_is_reported_and_seals_nothing() {
        let bytes = payload(512);

        // How many counted operations a clean seal performs, measured rather than assumed.
        let seal_ops = {
            let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
            let mut media = Fake::new();
            let mut scratch = stride();
            writer.append(capability, &mut media, &bytes).unwrap();
            let before = media.ops;
            writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::ZERO).unwrap();
            media.ops - before
        };
        assert_eq!(seal_ops, 7, "the payload sync plus the six of a gated WORK slot");

        for step in 1..=seal_ops {
            let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
            let mut media = Fake::new();
            let mut scratch = stride();
            writer.append(capability, &mut media, &bytes).unwrap();
            media.fail_at = Some(media.ops + step);
            let before = writer.capability();
            assert_eq!(
                writer.seal(before, &mut media, &mut scratch, DraftPartRef::ZERO),
                Err(WriteError::Media("injected")),
                "step {step} of the seal was injected but the seal completed",
            );
            assert_eq!(writer.state(), WriterState::Streaming, "step {step} sealed despite failing");
            assert_eq!(writer.capability(), before, "step {step}: a failed seal spent the capability");
            // The transaction is retryable: the payload is unchanged, so the same seal can run again.
            media.fail_at = None;
            assert!(writer.seal(before, &mut media, &mut scratch, DraftPartRef::ZERO).is_ok(), "step {step} retry");
        }
    }

    /// A draft child's seal is the one that mints a reference, and §7 admits it only on a sealed
    /// part row — which is what the record decoder enforces.
    #[test]
    fn a_draft_child_seals_with_its_minted_reference() {
        let bytes = payload(256);
        let mut child = intent(&bytes);
        child.subject = Subject::DraftPart;
        child.parent = OperationId::new(samples::OP_PARENT);
        child.part_key = 1;
        let (mut writer, capability) = GenerationWriter::begin::<&str>(child).unwrap();
        let mut media = Fake::new();
        let mut scratch = stride();
        writer.append(capability, &mut media, &bytes).unwrap();
        let record =
            writer.seal(writer.capability(), &mut media, &mut scratch, DraftPartRef::new(samples::PART_REF)).unwrap();
        assert_eq!(record.part_ref, DraftPartRef::new(samples::PART_REF));
        let slot: &[u8; SLOT_STRIDE] = media.work[..SLOT_STRIDE].try_into().unwrap();
        assert_eq!(WorkRecord::validate_slot(slot, 0).unwrap(), record);
    }

    /// An abort writes nothing at all: §6.2 makes the durable act the terminal record.
    #[test]
    fn abort_changes_nothing_on_the_card() {
        let bytes = payload(64);
        let (mut writer, capability) = GenerationWriter::begin::<&str>(intent(&bytes)).unwrap();
        let mut media = Fake::new();
        writer.append(capability, &mut media, &bytes).unwrap();
        media.log.clear();
        writer.abort::<&str>(writer.capability()).unwrap();
        assert_eq!(writer.state(), WriterState::Aborted);
        assert!(media.log.is_empty());
        assert_eq!(writer.append(capability, &mut media, &bytes), Err(WriteError::NotStreaming));
    }
}
