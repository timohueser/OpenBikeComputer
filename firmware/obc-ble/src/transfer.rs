//! The whole-object transfer state machine (spec §4.2 / §5) — a pure, radio-free core the board
//! feeds CoC bytes in and out of. There is **no per-chunk framing**: the [`TransferControl`]
//! descriptor announces the transfer on the control plane, the CoC carries exactly the payload
//! bytes, and one whole-object [`Crc32`] is verified at commit (spec §1 principle 2).
//!
//! Two directions, neither buffering the whole object. Interrupted transfers **restart rather
//! than resume in both directions** (spec §1 principle 4) — the descriptor's `offset` field is
//! shape-stability only and must be 0:
//!
//! - [`Receiver`] — the **upload** direction (app → device) and the receive half of the A5 echo:
//!   sink bytes with a running CRC, verify once at `total_len`, report [`TransferStatus::Committed`]
//!   or [`TransferStatus::CrcMismatch`].
//! - [`StreamSender`] — the **download** direction (device → app): emit the
//!   [`StreamSender::announce`] descriptor, then hand out `object[offset…]` in CoC-sized chunks with
//!   the whole-object CRC precomputed. The bytes never sit in this core: whether the object is a list
//!   built in a scratch buffer or a stored route/ride streamed off the SD card, the board supplies
//!   `total_len` + the precomputed whole-object CRC, reads each chunk from storage itself, and ticks
//!   the position through [`StreamSender::advance`].
//!
//! The board owns the trouble-host channel and the timing; this core owns the *sequencing + CRC +
//! typed outcome*, so all of that is `cargo test`-verified with an in-memory byte stream, exactly
//! like the app's `BLEChannel` under `swift test`.

use crate::crc32::Crc32;
use crate::descriptor::{ObjectType, Op, TransferControl, TransferResult, TransferStatus};

/// Why a [`Receiver`] / [`StreamSender`] couldn't be built from a descriptor. Semantic rejects the
/// board answers with a typed [`TransferResult`] (never a bare ATT failure, spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The descriptor's `op` doesn't match the constructor (an upload fed to [`StreamSender`], or vice versa).
    WrongOp,
    /// A non-zero `offset` — transfers restart whole in **both** directions (spec §1 principle
    /// 4); the field exists only for descriptor-shape stability and must be 0.
    NonZeroOffset,
}

/// The receive half of an upload (spec §4.2): sink CoC bytes with a running CRC and report a typed
/// outcome at `total_len`. Radio-free — the board calls [`push`] with whatever the CoC handed it
/// (any segmentation, spec §5) and reads the outcome when [`is_complete`] flips.
///
/// Uploads are **not resumable** (spec §1 principle 4): an interrupted upload is discarded and the
/// object re-sent from the start, so a receiver only ever starts fresh (`offset = 0`).
///
/// [`push`]: Receiver::push
/// [`is_complete`]: Receiver::is_complete
#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    object_id: u16,
    total_len: u32,
    expected_crc: u32,
    /// Absolute offset of the next expected byte — starts at 0 and advances as bytes arrive.
    position: u32,
    crc: Crc32,
}

impl Receiver {
    /// A fresh receiver from an upload descriptor. Rejects a non-upload op, or a non-zero `offset`
    /// (uploads restart, not resume — spec §1 principle 4; the board answers such a descriptor
    /// `error`).
    pub fn new(desc: &TransferControl) -> Result<Self, TransferError> {
        if desc.op != Op::Upload {
            return Err(TransferError::WrongOp);
        }
        if desc.offset != 0 {
            return Err(TransferError::NonZeroOffset);
        }
        Ok(Self {
            object_id: desc.object_id,
            total_len: desc.total_len,
            expected_crc: desc.crc32,
            position: 0,
            crc: Crc32::new(),
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

    /// Bytes received so far.
    pub fn committed_offset(&self) -> u32 {
        self.position
    }

    /// Bytes still expected before the transfer completes.
    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
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

/// The send half of a transfer (spec §4.2 download): the board reads `object[position…]` from
/// storage chunk by chunk and advances this tracker, which owns the announce descriptor and the
/// typed close. The whole-object `crc32` is precomputed by the caller (one read pass over storage) —
/// this core never sees the bytes, so it stays radio-free *and* storage-free.
#[derive(Clone, Copy, Debug)]
pub struct StreamSender {
    object_id: u16,
    ty: ObjectType,
    total_len: u32,
    /// Absolute offset of the next byte to send — starts at 0 (downloads restart, not resume).
    position: u32,
    crc: u32,
}

impl StreamSender {
    /// A sender for a download request (`op = Download`) of a stored object of `total_len` bytes
    /// whose whole-object CRC-32 is `crc32` — always streamed from the start (downloads restart,
    /// not resume). Rejects a non-download op or a non-zero `offset`.
    pub fn new(desc: &TransferControl, total_len: u32, crc32: u32) -> Result<Self, TransferError> {
        if desc.op != Op::Download {
            return Err(TransferError::WrongOp);
        }
        if desc.offset != 0 {
            return Err(TransferError::NonZeroOffset);
        }
        Ok(Self { object_id: desc.object_id, ty: desc.ty, total_len, position: 0, crc: crc32 })
    }

    /// The descriptor to notify before the bytes flow (spec §4.2): same 16 bytes as the request,
    /// `op = Download`, with `total_len` (the whole object) and `crc32` filled in.
    pub fn announce(&self) -> TransferControl {
        TransferControl {
            op: Op::Download,
            ty: self.ty,
            object_id: self.object_id,
            total_len: self.total_len,
            crc32: self.crc,
            offset: self.position,
        }
    }

    /// Absolute offset of the next byte to read from storage and send.
    pub fn position(&self) -> u32 {
        self.position
    }

    /// Bytes still to send.
    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
    }

    /// How many bytes the next storage read should fetch: `min(remaining, max)` for a CoC SDU of
    /// `max` bytes. `0` once complete.
    pub fn next_chunk_len(&self, max: usize) -> usize {
        core::cmp::min(self.remaining() as usize, max)
    }

    /// Record that `n` bytes (read at [`position`](Self::position)) were handed to the channel.
    /// Clamped to [`remaining`](Self::remaining) — the caller sizes reads with
    /// [`next_chunk_len`](Self::next_chunk_len), so a clamp only masks a caller bug in release.
    pub fn advance(&mut self, n: usize) {
        debug_assert!(n as u32 <= self.remaining());
        self.position += core::cmp::min(n as u32, self.remaining());
    }

    /// The whole object has been handed out.
    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// The explicit close (spec §4.2): [`Committed`](TransferStatus::Committed) once the whole object
    /// has been streamed, `committed_offset = total_len`. `None` while bytes remain.
    pub fn outcome(&self) -> Option<TransferResult> {
        self.is_complete().then(|| TransferResult::new(self.object_id, TransferStatus::Committed, self.total_len))
    }
}
