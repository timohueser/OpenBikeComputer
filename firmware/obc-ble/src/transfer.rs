//! The object transfer state machine — a pure, radio-free core the board feeds link bytes in and
//! out of. There is **no per-chunk framing**: the [`TransferControl`] descriptor announces the
//! transfer and the link carries exactly the payload bytes. Receivers normally verify one
//! whole-object [`Crc32`] at commit; a transport with sufficient link/media integrity may opt into
//! length-only receive while retaining the same descriptor and terminal result.
//!
//! Two directions, neither buffering the whole object. Interrupted transfers **restart rather than
//! resume in both directions** — there is no offset field on the wire (v2 dropped it), so a receiver
//! or sender only ever starts fresh:
//!
//! - [`Receiver`] — the **upload** direction (app → device): count bytes and, by default, fold a
//!   running CRC; at `total_len`, report [`TransferStatus::Committed`] or
//!   [`TransferStatus::CrcMismatch`].
//! - [`StreamSender`] — the **download** direction (device → app): emit the
//!   [`StreamSender::announce`] descriptor, then hand out `object[position…]` in CoC-sized chunks. The
//!   bytes never sit in this core: the board supplies `total_len` + the precomputed whole-object
//!   CRC, reads each chunk from storage itself, and ticks the position through
//!   [`StreamSender::advance`].

use crate::crc32::Crc32;
use crate::descriptor::{ObjectType, Op, TransferControl, TransferResult, TransferStatus};

/// Why a [`Receiver`] / [`StreamSender`] couldn't be built from a descriptor. The board answers a
/// semantic reject with a typed [`TransferResult`], never a bare ATT failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The descriptor's `op` doesn't match the constructor (an upload fed to [`StreamSender`], or vice versa).
    WrongOp,
}

/// The receive half of an upload: sink CoC bytes with a running CRC and report a typed outcome at
/// `total_len`. The board calls [`push`] with whatever the CoC handed it (any segmentation) and
/// reads the outcome when [`is_complete`] flips. Uploads are **not resumable** — an interrupted one
/// is discarded and re-sent from the start, so a receiver only ever starts fresh.
///
/// [`push`]: Receiver::push
/// [`is_complete`]: Receiver::is_complete
#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    object_id: u16,
    total_len: u32,
    expected_crc: u32,
    /// Absolute offset of the next expected byte.
    position: u32,
    crc: Crc32,
    verify_crc: bool,
}

impl Receiver {
    /// A fresh receiver from an upload descriptor. Rejects a non-upload op (uploads restart, not
    /// resume — there is no offset to reject in v2).
    pub fn new(desc: &TransferControl) -> Result<Self, TransferError> {
        Self::new_inner(desc, true)
    }

    /// A length-tracking receiver for a transport/storage path whose link and media already
    /// provide CRC/retry at lower cost than a second whole-object software digest on this CPU.
    ///
    /// This keeps the descriptor, byte-count clamp and typed close identical to [`new`](Self::new)
    /// but does not fold payload bytes or compare `crc32`. It is intended for the nRF54L USB map
    /// path, where USB 2.0 CRC/retry protects each packet, sEMMC CRC/ECC protects each stored block,
    /// and the format-specific commit validates the stored length/header before exposing the held
    /// magic. BLE and firmware-image uploads continue to use the full whole-object CRC.
    pub fn new_link_checked(desc: &TransferControl) -> Result<Self, TransferError> {
        Self::new_inner(desc, false)
    }

    fn new_inner(desc: &TransferControl, verify_crc: bool) -> Result<Self, TransferError> {
        if desc.op != Op::Upload {
            return Err(TransferError::WrongOp);
        }
        Ok(Self {
            object_id: desc.object_id,
            total_len: desc.total_len,
            expected_crc: desc.crc32,
            position: 0,
            crc: Crc32::new(),
            verify_crc,
        })
    }

    pub fn object_id(&self) -> u16 {
        self.object_id
    }

    pub fn total_len(&self) -> u32 {
        self.total_len
    }

    /// Bytes received so far.
    pub fn committed_offset(&self) -> u32 {
        self.position
    }

    /// Whether [`outcome`](Self::outcome) also requires the descriptor's whole-object CRC.
    pub fn verifies_crc(&self) -> bool {
        self.verify_crc
    }

    /// Bytes still expected before the transfer completes.
    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
    }

    /// Every announced byte has arrived (ready to verify).
    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// Feed link bytes, consuming up to [`remaining`](Receiver::remaining) and returning **how many
    /// were consumed**. A CRC-checking receiver also folds the consumed prefix into its digest.
    /// Only one transfer is ever in flight, so any surplus (`bytes.len() > consumed`) is an
    /// over-run the caller treats as a protocol error.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let take = core::cmp::min(bytes.len(), self.remaining() as usize);
        if self.verify_crc {
            self.crc.update(&bytes[..take]);
        }
        self.position += take as u32;
        take
    }

    /// Diagnostic probe: the running CRC as it stands (finalized) and the announced expectation.
    /// Read-only — the running digest is not consumed. Meaningful once
    /// [`is_complete`](Receiver::is_complete); before that the running value is a prefix CRC.
    pub fn crc_probe(&self) -> (u32, u32) {
        (self.crc.finalize(), self.expected_crc)
    }

    /// The terminal [`TransferResult`] once [`is_complete`](Receiver::is_complete): [`Committed`]
    /// if this receiver is link-checked or its whole-object CRC matches
    /// (`committed_offset = total_len`), else [`CrcMismatch`] (`committed_offset = 0` — nothing
    /// durable). `None` while bytes are still expected.
    ///
    /// [`Committed`]: TransferStatus::Committed
    /// [`CrcMismatch`]: TransferStatus::CrcMismatch
    pub fn outcome(&self) -> Option<TransferResult> {
        if !self.is_complete() {
            return None;
        }
        Some(if !self.verify_crc || self.crc.finalize() == self.expected_crc {
            TransferResult::new(self.object_id, TransferStatus::Committed, self.total_len)
        } else {
            TransferResult::new(self.object_id, TransferStatus::CrcMismatch, 0)
        })
    }
}

/// The **held-back magic** of a streamed upload that is written straight to its final filename
/// (issue #927): the first [`MAGIC_LEN`] payload bytes, withheld from the write and replayed only
/// once the whole object has verified.
///
/// Every other upload gets its atomicity from `UPLOAD.TMP` → copy-to-final-with-held-magic: the
/// temp is invisible to the catalog scans, and the copy patches the 4-byte format magic in as its
/// last write, so a power cut leaves either an invisible temp or a zero-magic final file that every
/// header read rejects. A **map** cannot afford the copy — it is hundreds of megabytes, so staging
/// then copying would double both the write time and the free space needed. So a map streams into
/// its final name directly and gets the *same* commit point another way: the file opens with four
/// zero bytes, the stream's own first four bytes are held here instead of written, and the commit
/// patches them in after the CRC and the header validate. The torn state is byte-for-byte the one
/// the copy path leaves — a zero-magic file the scan refuses and the boot sweep reclaims.
///
/// Segmentation-proof: the held bytes fill across as many `feed` calls as it takes, so a host that
/// opens with a 1-byte packet is handled exactly like one that opens with 512.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldMagic {
    bytes: [u8; MAGIC_LEN],
    /// How many of `bytes` are filled (0..=[`MAGIC_LEN`]).
    held: u8,
}

/// How many leading payload bytes [`HeldMagic`] withholds — the width of every OBC format magic
/// (`OBCM`, `OBCR`, `OBCU`).
pub const MAGIC_LEN: usize = 4;

impl HeldMagic {
    /// Nothing held yet — the state at the first payload byte.
    pub const fn new() -> Self {
        HeldMagic { bytes: [0; MAGIC_LEN], held: 0 }
    }

    /// Take the leading magic out of `bytes` and return **what the caller should write**: the
    /// remainder after the held prefix is complete, and an empty slice while it is still filling.
    /// Idempotent once full — every later chunk passes straight through.
    pub fn feed<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let want = MAGIC_LEN - self.held as usize;
        if want == 0 {
            return bytes;
        }
        let take = core::cmp::min(want, bytes.len());
        self.bytes[self.held as usize..self.held as usize + take].copy_from_slice(&bytes[..take]);
        self.held += take as u8;
        &bytes[take..]
    }

    /// The complete held magic, or `None` while fewer than [`MAGIC_LEN`] payload bytes have been
    /// fed (an object too short to have one — the announce guard rejects those long before here).
    pub const fn take(&self) -> Option<[u8; MAGIC_LEN]> {
        if self.held as usize == MAGIC_LEN {
            Some(self.bytes)
        } else {
            None
        }
    }
}

/// The send half of a transfer (download): the board reads `object[position…]` from storage chunk
/// by chunk and advances this tracker, which owns the announce descriptor and the typed close. The
/// whole-object `crc32` is precomputed by the caller — this core never sees the bytes, so it stays
/// radio-free *and* storage-free.
#[derive(Clone, Copy, Debug)]
pub struct StreamSender {
    object_id: u16,
    ty: ObjectType,
    total_len: u32,
    /// Absolute offset of the next byte to send (starts at 0; downloads restart, not resume).
    position: u32,
    crc: u32,
}

impl StreamSender {
    /// A sender for a download request of a stored object of `total_len` bytes whose whole-object
    /// CRC-32 is `crc32` — always streamed from the start. Rejects a non-download op.
    pub fn new(desc: &TransferControl, total_len: u32, crc32: u32) -> Result<Self, TransferError> {
        if desc.op != Op::Download {
            return Err(TransferError::WrongOp);
        }
        Ok(Self { object_id: desc.object_id, ty: desc.ty, total_len, position: 0, crc: crc32 })
    }

    /// The descriptor the board wraps in a [`StatusMessage::DownloadAnnounce`](crate::StatusMessage::DownloadAnnounce)
    /// and notifies on `status` before the bytes flow: the same 12 bytes as the request, `op =
    /// Download`, with `total_len` and `crc32` filled in.
    pub fn announce(&self) -> TransferControl {
        TransferControl {
            op: Op::Download,
            ty: self.ty,
            object_id: self.object_id,
            total_len: self.total_len,
            crc32: self.crc,
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
    /// Clamped to [`remaining`](Self::remaining), which in release only masks a caller that ignored
    /// [`next_chunk_len`](Self::next_chunk_len).
    pub fn advance(&mut self, n: usize) {
        debug_assert!(n as u32 <= self.remaining());
        self.position += core::cmp::min(n as u32, self.remaining());
    }

    /// The whole object has been handed out.
    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// The explicit close: [`Committed`](TransferStatus::Committed) once the whole object has been
    /// streamed, `committed_offset = total_len`. `None` while bytes remain.
    pub fn outcome(&self) -> Option<TransferResult> {
        self.is_complete().then(|| TransferResult::new(self.object_id, TransferStatus::Committed, self.total_len))
    }
}
