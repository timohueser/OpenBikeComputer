//! **§5.2's record framing**: `record_length u16` followed by exactly that many frame bytes, in
//! both directions, on both bulk endpoint pairs.
//!
//! This is the whole of what USB adds to `FLAT_Store_Protocol.md` §3, and the change from the v1
//! envelope it replaces is not the two bytes — it is that **packet boundaries carry no protocol
//! meaning**. The v1 control plane made one frame exactly one USB transfer, which capped a frame at
//! one max packet and made "which frame is this" a byte the transport had to invent. §5.2 instead
//! says a record may span packets and that records are never interleaved and never concatenated
//! without their prefixes, so a reader is a small reassembler over a byte stream and a writer is a
//! length in front of a frame.
//!
//! ## What the reader owes the rest of the adapter
//!
//! **One record at a time, and no read until the last one is released.** That is not a
//! simplification: §5 requires an adapter holding a stream frame to "withhold link credit — CoC
//! credits on BLE, ceasing to accept stream records on USB" rather than buffer a second one. A
//! reader that does not touch the endpoint while a record is out is exactly that: the bulk OUT
//! endpoint NAKs, and the host's own send loop is what stops.
//!
//! ## Errors end the link, because §5.2 says so
//!
//! "A zero, out-of-range, truncated or overrun record length is `invalidFrame` and resets that
//! record stream before teardown is reported to the engine." The adapter cannot *answer*
//! `invalidFrame` — §5 forbids it originating a frame, and the engine never saw the record — so what
//! it does is the rest of that sentence: it drops what it buffered and reports teardown. A peer that
//! has lost the record boundary cannot be re-synchronised by guessing where the next one starts.

use defmt::warn;
use embassy_usb::driver::{Endpoint as _, EndpointError, EndpointIn, EndpointOut};

use super::{EpIn, EpOut, MAX_PACKET};

/// Why a record stream ended. Every variant is a reason string the driver logs and tears down on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordEnd {
    /// The endpoint was disabled — an unplug, or a configuration change.
    LinkDown,
    /// §5.2's framing error: a length of zero, or one above this channel's ceiling.
    BadLength,
    /// A driver-level failure with the endpoint still up.
    Driver,
}

impl RecordEnd {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            RecordEnd::LinkDown => "link-down",
            RecordEnd::BadLength => "bad-record-length",
            RecordEnd::Driver => "endpoint",
        }
    }
}

/// The `record_length` prefix, in bytes.
pub(crate) const PREFIX_LEN: usize = 2;

/// **Reassembles §5.2 records off one bulk OUT endpoint.**
///
/// The buffer is sized by [`buffer_len`] so that a compaction always leaves room for one whole
/// armed read — which is what lets the reader make progress without ever refusing a transfer the
/// driver already armed.
pub(crate) struct RecordReader {
    ep: EpOut,
    buf: &'static mut [u8],
    /// Bytes in the buffer.
    filled: usize,
    /// Where the next unparsed record starts.
    at: usize,
    /// The largest record this channel accepts (§5.2).
    ceiling: usize,
    /// The endpoint's armed transfer size: what `read` will refuse a shorter buffer for.
    armed: usize,
}

/// The buffer one reader needs: a whole record, its prefix, and one armed read on top.
///
/// The `+ armed` term is what makes compaction sufficient rather than merely usual. A partial record
/// can be one byte short of a whole one, so the worst case after compaction is `PREFIX_LEN + ceiling
/// - 1` bytes held; the free tail must still take a full armed transfer, or the driver refuses the
/// read with `BufferOverflow` and the reader stalls with the peer still sending.
pub(crate) const fn buffer_len(ceiling: usize, armed: usize) -> usize {
    PREFIX_LEN + ceiling + armed
}

impl RecordReader {
    pub(crate) fn new(ep: EpOut, buf: &'static mut [u8], ceiling: usize, armed: usize) -> Self {
        debug_assert!(buf.len() >= buffer_len(ceiling, armed), "the reader's buffer cannot hold a record and a read");
        RecordReader { ep, buf, filled: 0, at: 0, ceiling, armed }
    }

    /// Park until the host has configured the interface. The endpoint is disabled before that and
    /// after every unplug, and this is the idle state of a cable-less device.
    pub(crate) async fn wait_enabled(&mut self) {
        self.ep.wait_enabled().await;
    }

    /// Forget everything buffered — a new configuration starts a new record stream.
    pub(crate) fn reset(&mut self) {
        self.filled = 0;
        self.at = 0;
    }

    /// **The next whole record.**
    ///
    /// The returned slice aliases this reader's buffer and is valid until the next call, which is
    /// the contract the caller keeps by holding one record at a time — and, on the stream channel,
    /// is also §5's credit withholding.
    pub(crate) async fn next(&mut self) -> Result<&'static [u8], RecordEnd> {
        loop {
            if let Some(record) = self.take()? {
                return Ok(record);
            }
            // Make room for one whole armed read before asking for one.
            if self.buf.len() - self.filled < self.armed {
                self.buf.copy_within(self.at..self.filled, 0);
                self.filled -= self.at;
                self.at = 0;
            }
            match self.ep.read(&mut self.buf[self.filled..]).await {
                Ok(0) => {}
                Ok(n) => self.filled += n,
                Err(EndpointError::Disabled) => return Err(RecordEnd::LinkDown),
                Err(e) => {
                    // Not a disable — a driver-level failure with the endpoint still up. The driver
                    // backs off before re-accepting; re-arming here would hot-spin against a
                    // persistent one and starve the ride loop on this cooperative executor.
                    warn!("usb: [rec] read failed: {:?}", defmt::Debug2Format(&e));
                    return Err(RecordEnd::Driver);
                }
            }
        }
    }

    /// One whole record out of what is already buffered, if there is one.
    fn take(&mut self) -> Result<Option<&'static [u8]>, RecordEnd> {
        let held = self.filled - self.at;
        if held < PREFIX_LEN {
            return Ok(None);
        }
        let len = usize::from(u16::from_le_bytes([self.buf[self.at], self.buf[self.at + 1]]));
        if len == 0 || len > self.ceiling {
            warn!("usb: [rec] record length {} is outside this channel's ceiling {}", len, self.ceiling);
            return Err(RecordEnd::BadLength);
        }
        if held < PREFIX_LEN + len {
            return Ok(None);
        }
        let start = self.at + PREFIX_LEN;
        self.at += PREFIX_LEN + len;
        // SAFETY: the slice aliases `self.buf`, which this reader owns for the life of the image.
        // It is invalidated only by the next `next`/`reset`, which is exactly the caller's
        // one-record-at-a-time contract — the same window the BLE adapter's staged record lives in.
        Ok(Some(unsafe { core::slice::from_raw_parts(self.buf.as_ptr().add(start), len) }))
    }
}

/// **Writes §5.2 records to one bulk IN endpoint.**
///
/// The prefix goes out as its own transfer rather than being copied in front of the frame, and that
/// is a consequence of where the frame lives: it is the engine's reaction buffer, lent through the
/// storage queue, and the two bytes in front of it would have to be either a second copy of a
/// 4 KiB record or a reserved head the engine would have to be taught about. Packet boundaries
/// carry no protocol meaning here (§5.2), and this endpoint has exactly one writer — the driver —
/// so nothing can interleave between the two transfers.
pub(crate) struct RecordWriter {
    ep: EpIn,
}

impl RecordWriter {
    pub(crate) fn new(ep: EpIn) -> Self {
        RecordWriter { ep }
    }

    /// Send one record. `false` means the endpoint failed and the link is over.
    pub(crate) async fn send(&mut self, frame: &[u8]) -> bool {
        let Ok(len) = u16::try_from(frame.len()) else {
            warn!("usb: [rec] a {}-byte frame cannot carry a u16 length prefix — dropping", frame.len());
            return false;
        };
        if self.write(&len.to_le_bytes()).await.is_err() {
            return false;
        }
        // One call is one packet on this driver, so a record wider than a packet goes out as
        // several. The peer reassembles by the length above and never by transfer boundaries.
        for chunk in frame.chunks(MAX_PACKET as usize) {
            if self.write(chunk).await.is_err() {
                return false;
            }
        }
        true
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), EndpointError> {
        self.ep.write(bytes).await.inspect_err(|e| {
            warn!("usb: [rec] write failed: {:?}", defmt::Debug2Format(e));
        })
    }
}
