//! The object transfer state machine: a pure, radio-free core the board feeds link bytes in and out
//! of. There is no per-chunk framing — the [`TransferControl`] descriptor announces the transfer and
//! the link carries exactly the payload bytes. An interrupted transfer restarts in both directions,
//! because the wire has no offset field.
//!
//! [`Receiver`] is the upload direction: count bytes, fold a running [`Crc32`] by default, and
//! report a terminal status at `total_len`. [`StreamSender`] is the download direction: emit the
//! announce descriptor, then hand out `object[position…]` in CoC-sized chunks. Neither buffers the
//! object; the board reads each chunk from storage itself.

use crate::crc32::Crc32;
use crate::descriptor::{ObjectType, Op, TransferControl, TransferResult, TransferStatus};

/// Why a [`Receiver`] or a [`StreamSender`] could not be built from a descriptor. The board answers
/// a semantic reject with a typed [`TransferResult`], never a bare ATT failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The descriptor's `op` does not match the constructor.
    WrongOp,
}

/// The receive half of an upload: sink CoC bytes with a running CRC and report a typed outcome at
/// `total_len`. The board pushes whatever the CoC handed it, in any segmentation.
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
    pub fn new(desc: &TransferControl) -> Result<Self, TransferError> {
        Self::new_inner(desc, true)
    }

    /// A receiver that counts bytes but does not fold or compare a CRC, for a path whose link and
    /// media already give CRC and retry: the USB map path, where USB protects each packet, sEMMC
    /// protects each stored block, and the commit validates the stored header. BLE and
    /// firmware-image uploads keep the whole-object CRC.
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

    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
    }

    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// Consumes up to [`remaining`](Receiver::remaining) bytes and returns how many it took. Only
    /// one transfer is ever in flight, so a surplus is an over-run the caller must treat as a
    /// protocol error.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let take = core::cmp::min(bytes.len(), self.remaining() as usize);
        if self.verify_crc {
            self.crc.update(&bytes[..take]);
        }
        self.position += take as u32;
        take
    }

    /// The running CRC, finalized, and the announced expectation. The digest is not consumed, and
    /// the value is a prefix CRC until [`is_complete`](Receiver::is_complete).
    pub fn crc_probe(&self) -> (u32, u32) {
        (self.crc.finalize(), self.expected_crc)
    }

    /// The terminal result once [`is_complete`](Receiver::is_complete): `Committed` with
    /// `committed_offset = total_len`, or `CrcMismatch` with `committed_offset = 0` because nothing
    /// is durable. `None` while bytes are still expected.
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

/// The first [`MAGIC_LEN`] payload bytes of an upload that streams straight into its final
/// filename. The file opens with four zero bytes, these bytes are held back, and the commit patches
/// them in after the CRC and the header validate. A power cut therefore leaves a zero-magic file
/// that every header read rejects and the boot sweep reclaims. A map takes this path because it is
/// far too large to stage in a temp file and copy, which is how every other upload commits.
///
/// The held bytes fill across as many `feed` calls as it takes, so a 1-byte first packet behaves
/// like a 512-byte one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldMagic {
    bytes: [u8; MAGIC_LEN],
    /// How many of `bytes` are filled (0..=[`MAGIC_LEN`]).
    held: u8,
}

/// The width of every OBC format magic (`OBCM`, `OBCR`, `OBCU`).
pub const MAGIC_LEN: usize = 4;

impl HeldMagic {
    pub const fn new() -> Self {
        HeldMagic { bytes: [0; MAGIC_LEN], held: 0 }
    }

    /// Takes the leading magic out of `bytes` and returns what the caller must write: the
    /// remainder once the held prefix is complete, and an empty slice while it still fills. Once
    /// full, every later chunk passes straight through.
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

    /// `None` while fewer than [`MAGIC_LEN`] payload bytes have been fed.
    pub const fn take(&self) -> Option<[u8; MAGIC_LEN]> {
        if self.held as usize == MAGIC_LEN {
            Some(self.bytes)
        } else {
            None
        }
    }
}

/// The send half of a download: the board reads `object[position…]` from storage chunk by chunk and
/// advances this tracker, which owns the announce descriptor and the typed close. The caller
/// precomputes the whole-object CRC, so this core never sees the bytes.
#[derive(Clone, Copy, Debug)]
pub struct StreamSender {
    object_id: u16,
    ty: ObjectType,
    total_len: u32,
    /// Absolute offset of the next byte to send.
    position: u32,
    crc: u32,
}

impl StreamSender {
    /// `crc32` is the whole-object CRC of the stored object. Rejects a non-download op.
    pub fn new(desc: &TransferControl, total_len: u32, crc32: u32) -> Result<Self, TransferError> {
        if desc.op != Op::Download {
            return Err(TransferError::WrongOp);
        }
        Ok(Self { object_id: desc.object_id, ty: desc.ty, total_len, position: 0, crc: crc32 })
    }

    /// The descriptor the board notifies on `status` before the bytes flow: the same 12 bytes as
    /// the request, with `total_len` and `crc32` filled in.
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

    pub fn remaining(&self) -> u32 {
        self.total_len - self.position
    }

    /// How many bytes the next storage read must fetch for a CoC SDU of `max` bytes.
    pub fn next_chunk_len(&self, max: usize) -> usize {
        core::cmp::min(self.remaining() as usize, max)
    }

    /// Record that `n` bytes, read at [`position`](Self::position), went to the channel. Clamped to
    /// [`remaining`](Self::remaining).
    pub fn advance(&mut self, n: usize) {
        debug_assert!(n as u32 <= self.remaining());
        self.position += core::cmp::min(n as u32, self.remaining());
    }

    pub fn is_complete(&self) -> bool {
        self.position == self.total_len
    }

    /// [`Committed`](TransferStatus::Committed) with `committed_offset = total_len` once the whole
    /// object is streamed; `None` while bytes remain.
    pub fn outcome(&self) -> Option<TransferResult> {
        self.is_complete().then(|| TransferResult::new(self.object_id, TransferStatus::Committed, self.total_len))
    }
}
