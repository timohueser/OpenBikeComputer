//! The whole-object transfer state machine (spec §4.2 / §5) — a pure, radio-free core the board
//! feeds CoC bytes in and out of. There is **no per-chunk framing**: the [`TransferControl`]
//! descriptor announces the transfer on the control plane, the CoC carries exactly the payload
//! bytes, and one whole-object [`Crc32`] is verified at commit (spec §1 principle 2).
//!
//! Two directions, two types, both offset-resumable and neither buffering the whole object:
//!
//! - [`Receiver`] — the **upload** direction (app → device) and the receive half of the A5 echo:
//!   sink bytes with a running CRC, verify once at `total_len`, report [`TransferStatus::Committed`]
//!   or [`TransferStatus::CrcMismatch`]. Resume by seeding the committed-prefix CRC ([`Receiver::resumed`]).
//! - [`Sender`] — the **download** direction (device → app): emit the [`Sender::announce`] descriptor,
//!   then hand out `object[offset…]` in CoC-sized chunks with the whole-object CRC precomputed.
//!
//! The board owns the trouble-host channel and the timing; this core owns the *sequencing + CRC +
//! typed outcome*, so all of that is `cargo test`-verified with an in-memory byte stream, exactly
//! like the app's `BLEChannel` under `swift test`.

use crate::crc32::Crc32;
use crate::descriptor::{ObjectType, Op, TransferControl, TransferResult, TransferStatus};

/// Why a [`Receiver`] / [`Sender`] couldn't be built from a descriptor. Semantic rejects the board
/// answers with a typed [`TransferResult`] (never a bare ATT failure, spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The descriptor's `op` doesn't match the constructor (an upload fed to [`Sender`], or vice versa).
    WrongOp,
    /// `offset > total_len` — a nonsensical resume anchor (spec §4.2 "past `total_len`").
    OffsetPastTotal,
}

/// Alias kept for the crate's public surface — receiver construction shares [`TransferError`].
pub type ReceiverError = TransferError;

/// The receive half of a transfer (spec §4.2 upload): sink CoC bytes with a running CRC and report a
/// typed outcome at `total_len`. Radio-free — the board calls [`push`] with whatever the CoC handed
/// it (any segmentation, spec §5) and reads the outcome when [`is_complete`] flips.
///
/// **Resume** (spec §4.2): a drop costs only the un-flushed tail. The device keeps its running CRC
/// across the resume — snapshot [`crc`] at the committed offset, and rebuild with [`Receiver::resumed`]
/// from the new descriptor's `offset` and that snapshot. (The A5 echo has no storage, so its
/// committed offset after a drop is 0 and a resume just restarts fresh; real storage resumes land at A6.)
///
/// [`push`]: Receiver::push
/// [`is_complete`]: Receiver::is_complete
/// [`crc`]: Receiver::crc
#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    object_id: u16,
    total_len: u32,
    expected_crc: u32,
    /// Absolute offset of the next expected byte — starts at the descriptor's `offset`, the
    /// committed byte count and the resume anchor.
    position: u32,
    crc: Crc32,
}

impl Receiver {
    /// A **fresh** receiver from an upload descriptor (`offset` must be 0 — a nonzero offset needs
    /// the committed-prefix CRC, so use [`Receiver::resumed`]). Rejects a non-upload op.
    pub fn new(desc: &TransferControl) -> Result<Self, TransferError> {
        Self::resumed(desc, Crc32::new())
    }

    /// A **resumed** receiver: continue an upload from `desc.offset` with `prefix_crc` = the running
    /// [`Crc32`] over the already-committed `object[..offset]` bytes. For a fresh start pass
    /// `Crc32::new()` with `offset = 0` (that's what [`Receiver::new`] does).
    pub fn resumed(desc: &TransferControl, prefix_crc: Crc32) -> Result<Self, TransferError> {
        if desc.op != Op::Upload {
            return Err(TransferError::WrongOp);
        }
        if desc.offset > desc.total_len {
            return Err(TransferError::OffsetPastTotal);
        }
        Ok(Self {
            object_id: desc.object_id,
            total_len: desc.total_len,
            expected_crc: desc.crc32,
            position: desc.offset,
            crc: prefix_crc,
        })
    }

    /// The object id from the descriptor (echoed in the outcome; A6 reassigns a fresh-upload id).
    pub fn object_id(&self) -> u16 {
        self.object_id
    }

    /// The announced object size.
    pub fn total_len(&self) -> u32 {
        self.total_len
    }

    /// Bytes received so far — the committed offset / resume anchor.
    pub fn committed_offset(&self) -> u32 {
        self.position
    }

    /// Bytes still expected before the transfer completes.
    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
    }

    /// The running CRC — snapshot it at the committed offset to seed a [`Receiver::resumed`].
    pub fn crc(&self) -> Crc32 {
        self.crc
    }

    /// Every announced byte has arrived (ready to verify).
    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// Feed CoC bytes. Folds up to [`remaining`](Receiver::remaining) of them into the running CRC
    /// and advances the committed offset, returning **how many were consumed**. A well-behaved link
    /// hands over exactly `remaining` across the transfer; any surplus (`bytes.len() > consumed`)
    /// after completion is an over-run the caller treats as a protocol error — there is only ever
    /// one transfer in flight (spec §4.1), so no trailing bytes belong to anything.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let take = core::cmp::min(bytes.len(), self.remaining() as usize);
        self.crc.update(&bytes[..take]);
        self.position += take as u32;
        take
    }

    /// The terminal [`TransferResult`] once [`is_complete`](Receiver::is_complete): [`Committed`] if
    /// the whole-object CRC matches (`committed_offset = total_len`), else [`CrcMismatch`]
    /// (`committed_offset = 0` — nothing durable, spec §4.2). `None` while bytes are still expected.
    ///
    /// [`Committed`]: TransferStatus::Committed
    /// [`CrcMismatch`]: TransferStatus::CrcMismatch
    pub fn outcome(&self) -> Option<TransferResult> {
        if !self.is_complete() {
            return None;
        }
        Some(if self.crc.finalize() == self.expected_crc {
            TransferResult::new(self.object_id, TransferStatus::Committed, self.total_len)
        } else {
            TransferResult::new(self.object_id, TransferStatus::CrcMismatch, 0)
        })
    }
}

/// The send half of a transfer (spec §4.2 download): announce the object's size + CRC, then hand out
/// `object[offset…]` in CoC-sized chunks. Radio-free — the board notifies [`announce`], then writes
/// each [`next_chunk`] to the CoC. Borrows the object (from storage in A7, or any slice in a test),
/// so nothing is copied.
///
/// [`announce`]: Sender::announce
/// [`next_chunk`]: Sender::next_chunk
#[derive(Clone, Copy, Debug)]
pub struct Sender<'a> {
    object_id: u16,
    ty: ObjectType,
    object: &'a [u8],
    /// Absolute offset of the next byte to send — starts at the requested `offset`.
    position: usize,
    crc: u32,
}

impl<'a> Sender<'a> {
    /// A sender for a download request (`op = Download`). `object` is the whole stored object; the
    /// request's `offset` is where to resume from (spec §4.2 download resume). Rejects a non-download
    /// op or an offset past the object.
    pub fn new(desc: &TransferControl, object: &'a [u8]) -> Result<Self, TransferError> {
        if desc.op != Op::Download {
            return Err(TransferError::WrongOp);
        }
        if desc.offset as usize > object.len() {
            return Err(TransferError::OffsetPastTotal);
        }
        Ok(Self {
            object_id: desc.object_id,
            ty: desc.ty,
            object,
            position: desc.offset as usize,
            crc: Crc32::checksum(object),
        })
    }

    /// The descriptor to notify before the bytes flow (spec §4.2): same 16 bytes as the request,
    /// `op = Download`, with `total_len` (the whole object) and `crc32` filled in. The `offset` is
    /// where streaming resumes from, so the app knows how many bytes to expect (`total_len - offset`).
    pub fn announce(&self) -> TransferControl {
        TransferControl {
            op: Op::Download,
            ty: self.ty,
            object_id: self.object_id,
            total_len: self.object.len() as u32,
            crc32: self.crc,
            offset: self.position as u32,
        }
    }

    /// Bytes still to send.
    pub fn remaining(&self) -> usize {
        self.object.len() - self.position
    }

    /// The whole object has been handed out.
    pub fn is_complete(&self) -> bool {
        self.position == self.object.len()
    }

    /// The next CoC chunk (up to `max` bytes), advancing the offset; `None` when the object is fully
    /// sent. `max` is the write granularity (one CoC SDU, ~244 bytes on a 2M-PHY + DLE link).
    pub fn next_chunk(&mut self, max: usize) -> Option<&'a [u8]> {
        if self.is_complete() || max == 0 {
            return None;
        }
        let end = core::cmp::min(self.position + max, self.object.len());
        let chunk = &self.object[self.position..end];
        self.position = end;
        Some(chunk)
    }

    /// The explicit close (spec §4.2): [`Committed`](TransferStatus::Committed) once the whole object
    /// has been streamed, `committed_offset = total_len`. `None` while bytes remain.
    pub fn outcome(&self) -> Option<TransferResult> {
        self.is_complete()
            .then(|| TransferResult::new(self.object_id, TransferStatus::Committed, self.object.len() as u32))
    }
}
