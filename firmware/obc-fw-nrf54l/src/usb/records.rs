//! USB record framing: `record_length u32`, that many frame bytes, then zero padding to a
//! four-byte boundary, in both directions on both bulk endpoint pairs. Packet boundaries carry no
//! protocol meaning, so a record can span packets.
//!
//! The reader holds one record at a time and does not read again until that record is released.
//! This is the link credit the protocol asks for: the bulk OUT endpoint NAKs and the host's send
//! loop stops. A framing error ends the record stream, because a peer that lost the record
//! boundary cannot resynchronise.

use defmt::warn;
use embassy_usb::driver::{Endpoint as _, EndpointError, EndpointIn, EndpointOut};
use obc_link::flat::{padded_record_len, Reassembler, RecordFault};

use super::{EpIn, EpOut, MAX_PACKET};

pub(crate) use obc_link::flat::record_buffer_len as buffer_len;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordEnd {
    /// The endpoint is disabled: an unplug, or a configuration change.
    LinkDown,
    /// A bad record length, or non-zero alignment padding.
    BadFraming,
    /// A driver-level failure with the endpoint still up.
    Driver,
}

impl RecordEnd {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            RecordEnd::LinkDown => "link-down",
            RecordEnd::BadFraming => "bad-record-framing",
            RecordEnd::Driver => "endpoint",
        }
    }
}

/// Reassembles records off one bulk OUT endpoint. [`buffer_len`] sizes the buffer so that a
/// compaction always leaves room for one whole armed read.
pub(crate) struct RecordReader {
    ep: EpOut,
    buf: &'static mut [u8],
    frames: Reassembler,
    /// The endpoint's armed transfer size. `read` refuses a shorter buffer.
    armed: usize,
}

impl RecordReader {
    pub(crate) fn new(ep: EpOut, buf: &'static mut [u8], ceiling: usize, armed: usize) -> Self {
        debug_assert!(buf.len() >= buffer_len(ceiling, armed), "the reader's buffer cannot hold a record and a read");
        RecordReader { ep, buf, frames: Reassembler::new(ceiling), armed }
    }

    /// Park until the host configures the interface. The endpoint is disabled before that, and
    /// after every unplug.
    pub(crate) async fn wait_enabled(&mut self) {
        self.ep.wait_enabled().await;
    }

    /// Forget everything buffered. A new configuration starts a new record stream, and a framing
    /// fault must reset the stream before teardown reaches the engine.
    pub(crate) fn reset(&mut self) {
        self.frames.reset();
    }

    /// The next whole record. The returned slice aliases this reader's buffer and is valid until
    /// the next call, so the caller must hold one record at a time.
    pub(crate) async fn next(&mut self) -> Result<&'static [u8], RecordEnd> {
        loop {
            match self.frames.take(self.buf) {
                Ok(Some((start, len))) => {
                    // SAFETY: the slice aliases `self.buf`, which this reader owns for the life of
                    // the image. Only the next `next` or `reset` invalidates it, which is the
                    // caller's one-record-at-a-time contract.
                    return Ok(unsafe { core::slice::from_raw_parts(self.buf.as_ptr().add(start), len) });
                }
                Ok(None) => {}
                Err(fault) => {
                    match fault {
                        RecordFault::ZeroLength => warn!("usb: [rec] a zero record length is not a record"),
                        RecordFault::OverCeiling { declared, ceiling } => {
                            warn!("usb: [rec] record length {} is above this channel's ceiling {}", declared, ceiling)
                        }
                        RecordFault::NonZeroPadding => warn!("usb: [rec] record padding is not zero"),
                    }
                    return Err(RecordEnd::BadFraming);
                }
            }
            let at = self.frames.read_offset(self.buf, self.armed);
            match self.ep.read(&mut self.buf[at..]).await {
                Ok(0) => {}
                Ok(n) => self.frames.filled(n),
                Err(EndpointError::Disabled) => return Err(RecordEnd::LinkDown),
                Err(e) => {
                    // The driver backs off before it accepts again. To re-arm here would hot-spin
                    // on a persistent failure and starve the ride loop on this cooperative executor.
                    warn!("usb: [rec] read failed: {:?}", defmt::Debug2Format(&e));
                    return Err(RecordEnd::Driver);
                }
            }
        }
    }
}

/// Writes records to one bulk IN endpoint. The length prefix goes out as its own transfer,
/// because the frame is the engine's reaction buffer and cannot get four more bytes in front of
/// it. Packet boundaries carry no protocol meaning, and the driver is the only writer here.
pub(crate) struct RecordWriter {
    ep: EpIn,
}

impl RecordWriter {
    pub(crate) fn new(ep: EpIn) -> Self {
        RecordWriter { ep }
    }

    /// Send one record. `false` means the endpoint failed and the link is over.
    pub(crate) async fn send(&mut self, frame: &[u8]) -> bool {
        let Ok(len) = u32::try_from(frame.len()) else {
            warn!("usb: [rec] a {}-byte frame cannot carry a u32 length prefix — dropping", frame.len());
            return false;
        };
        if self.write(&len.to_le_bytes()).await.is_err() {
            return false;
        }
        // One call is one packet on this driver, so a record wider than a packet goes out as
        // several.
        for chunk in frame.chunks(MAX_PACKET as usize) {
            if self.write(chunk).await.is_err() {
                return false;
            }
        }
        let padding = padded_record_len(frame.len()) - frame.len();
        if padding != 0 && self.write(&[0; 3][..padding]).await.is_err() {
            return false;
        }
        true
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), EndpointError> {
        self.ep.write(bytes).await.inspect_err(|e| {
            warn!("usb: [rec] write failed: {:?}", defmt::Debug2Format(e));
        })
    }
}
