//! **§5.2's record reassembly**, as a pure state machine over a caller's buffer.
//!
//! The USB binding frames every record as `record_length u16` followed by exactly that many frame
//! bytes, and — the part that makes this a state machine rather than a length check — **packet
//! boundaries carry no protocol meaning**. A record may span packets; several may arrive in one
//! read; a read may end mid-length-prefix. The v1 envelope made one frame exactly one USB transfer
//! and needed none of this.
//!
//! It lives in `obc-link` for the same reason [`Ceilings`](super::Ceilings) and
//! [`Admission`](super::Admission) do: it is a **rule of the binding**, stated in §5.2, and the
//! board crate is bare metal with no test harness in CI. A rule written there would be a rule
//! nothing checks. The endpoint, the buffer and the `unsafe` that hands a record out as `'static`
//! stay on the board, where they belong; the arithmetic is here, where it is tested.

/// Why a record stream could not continue — §5.2's "a zero, out-of-range, truncated or overrun
/// record length is `invalidFrame` and resets that record stream".
///
/// Truncation is not here, and that is not an omission: a short read is indistinguishable from a
/// record still arriving, so it is [`Reassembler::take`] answering `None` rather than a fault. It
/// becomes a fault only when the endpoint dies, which is the transport's fact and not this one's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordFault {
    /// A zero `record_length`. §5.2 forbids it outright.
    ZeroLength,
    /// A `record_length` above this channel's ceiling.
    OverCeiling {
        /// What the prefix declared.
        declared: usize,
        /// What the channel accepts.
        ceiling: usize,
    },
}

/// The `record_length` prefix, in bytes.
pub const PREFIX_LEN: usize = 2;

/// **The buffer one reader needs**: a whole record, its prefix, and one armed read on top.
///
/// The `+ armed` term is what makes compaction *sufficient* rather than merely usual. A partial
/// record can be one byte short of a whole one, so the worst case after compaction is
/// `PREFIX_LEN + ceiling - 1` bytes held — and the free tail must still take a full armed transfer,
/// or the driver refuses the read and the reader stalls with the peer still sending.
pub const fn buffer_len(ceiling: usize, armed: usize) -> usize {
    PREFIX_LEN + ceiling + armed
}

/// Reassembles §5.2 records out of a byte stream. Owns no bytes — the caller's buffer is passed in,
/// because on the device it is a `static` the endpoint reads into directly.
#[derive(Debug, Clone, Copy)]
pub struct Reassembler {
    ceiling: usize,
    /// Bytes in the buffer.
    filled: usize,
    /// Where the next unparsed record starts.
    at: usize,
}

impl Reassembler {
    /// A reassembler for a channel with this record ceiling.
    pub const fn new(ceiling: usize) -> Self {
        Reassembler { ceiling, filled: 0, at: 0 }
    }

    /// Forget everything buffered — a new configuration starts a new record stream, and §5.2 also
    /// requires this after a framing fault, *before* teardown is reported to the engine.
    pub fn reset(&mut self) {
        self.filled = 0;
        self.at = 0;
    }

    /// Bytes currently held. Diagnostics and tests.
    pub const fn buffered(&self) -> usize {
        self.filled - self.at
    }

    /// **Where the next read should land**, compacting first if the free tail cannot take a whole
    /// armed transfer. Returns the offset into `buf`; the caller reads into `buf[offset..]`.
    ///
    /// Compaction is a `copy_within` of at most one partial record, and it happens only when it has
    /// to — a steady stream of whole records never moves a byte.
    pub fn read_offset(&mut self, buf: &mut [u8], armed: usize) -> usize {
        if buf.len() - self.filled < armed {
            buf.copy_within(self.at..self.filled, 0);
            self.filled -= self.at;
            self.at = 0;
        }
        self.filled
    }

    /// Record that `n` bytes landed at the offset [`read_offset`](Self::read_offset) gave.
    pub fn filled(&mut self, n: usize) {
        self.filled += n;
    }

    /// **The next whole record**, as `(start, len)` into the caller's buffer.
    ///
    /// `Ok(None)` means "not yet" — more bytes are needed, and the caller should read again. It is
    /// deliberately not a fault: a record still arriving and a record that will never finish are
    /// the same thing until the endpoint says otherwise.
    pub fn take(&mut self, buf: &[u8]) -> Result<Option<(usize, usize)>, RecordFault> {
        if self.buffered() < PREFIX_LEN {
            return Ok(None);
        }
        let len = usize::from(u16::from_le_bytes([buf[self.at], buf[self.at + 1]]));
        if len == 0 {
            return Err(RecordFault::ZeroLength);
        }
        if len > self.ceiling {
            return Err(RecordFault::OverCeiling { declared: len, ceiling: self.ceiling });
        }
        if self.buffered() < PREFIX_LEN + len {
            return Ok(None);
        }
        let start = self.at + PREFIX_LEN;
        self.at += PREFIX_LEN + len;
        Ok(Some((start, len)))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    use super::*;

    /// One record delivered as several reads — the property the v1 envelope did not have and §5.2
    /// requires. The reads deliberately split the *length prefix* as well as the body, because a
    /// reader that only handled a split body would pass a test that split at byte four and fail on
    /// a real endpoint.
    #[test]
    fn a_record_spanning_reads_is_reassembled_whatever_the_split() {
        const CEILING: usize = 64;
        let record: Vec<u8> =
            core::iter::once(40u8).chain(core::iter::once(0u8)).chain((0..40u8).map(|b| b.wrapping_add(1))).collect();
        for split in 1..record.len() {
            let mut buf = vec![0u8; buffer_len(CEILING, 16)];
            let mut r = Reassembler::new(CEILING);
            let mut fed = 0;
            let mut got = None;
            for chunk in [&record[..split], &record[split..]] {
                let at = r.read_offset(&mut buf, chunk.len());
                buf[at..at + chunk.len()].copy_from_slice(chunk);
                r.filled(chunk.len());
                fed += chunk.len();
                got = r.take(&buf).expect("well formed");
                if got.is_some() {
                    break;
                }
            }
            let (start, len) = got.unwrap_or_else(|| panic!("split at {split} never completed after {fed} B"));
            assert_eq!(len, 40, "split at {split}");
            assert_eq!(&buf[start..start + len], &record[2..], "split at {split}");
        }
    }

    /// Several whole records in one read, and a read that ends exactly on a record boundary — the
    /// "exact multiple" case, where an off-by-one leaves a phantom empty record or eats the next one.
    #[test]
    fn several_records_in_one_read_come_out_one_at_a_time() {
        const CEILING: usize = 64;
        let mut stream: Vec<u8> = Vec::new();
        for len in [8usize, 16, 8] {
            stream.extend_from_slice(&(len as u16).to_le_bytes());
            stream.extend(core::iter::repeat_n(len as u8, len));
        }
        let mut buf = vec![0u8; buffer_len(CEILING, stream.len())];
        let mut r = Reassembler::new(CEILING);
        let at = r.read_offset(&mut buf, stream.len());
        buf[at..at + stream.len()].copy_from_slice(&stream);
        r.filled(stream.len());

        for expected in [8usize, 16, 8] {
            let (start, len) = r.take(&buf).expect("well formed").expect("a whole record");
            assert_eq!(len, expected);
            assert!(buf[start..start + len].iter().all(|&b| b as usize == expected));
        }
        assert_eq!(r.take(&buf), Ok(None), "the read ended exactly on a boundary");
        assert_eq!(r.buffered(), 0);
    }

    /// §5.2's two framing faults, told apart. A zero length and an over-ceiling one are both
    /// `invalidFrame` on the wire, but a device that cannot tell them apart in its log cannot tell a
    /// broken client from a client talking to the wrong channel.
    #[test]
    fn a_zero_and_an_over_ceiling_length_are_distinguishable_faults() {
        const CEILING: usize = 64;
        let mut buf = vec![0u8; buffer_len(CEILING, 16)];
        let mut r = Reassembler::new(CEILING);
        buf[0..2].copy_from_slice(&0u16.to_le_bytes());
        r.filled(2);
        assert_eq!(r.take(&buf), Err(RecordFault::ZeroLength));

        let mut r = Reassembler::new(CEILING);
        buf[0..2].copy_from_slice(&65u16.to_le_bytes());
        r.filled(2);
        assert_eq!(r.take(&buf), Err(RecordFault::OverCeiling { declared: 65, ceiling: CEILING }));

        // …and a length exactly at the ceiling is legal, which is the boundary a `>=` would break.
        let mut r = Reassembler::new(CEILING);
        buf[0..2].copy_from_slice(&(CEILING as u16).to_le_bytes());
        r.filled(2 + CEILING);
        assert_eq!(r.take(&buf), Ok(Some((2, CEILING))));
    }

    /// Compaction always leaves room for a whole armed read — the property [`buffer_len`] is sized
    /// for, at its worst case: a partial record one byte short of the ceiling.
    #[test]
    fn compaction_always_leaves_room_for_one_armed_read() {
        const CEILING: usize = 64;
        const ARMED: usize = 16;
        let mut buf = vec![0u8; buffer_len(CEILING, ARMED)];
        let mut r = Reassembler::new(CEILING);

        // Fill the buffer with whole records, then a partial one as long as it can be.
        let at = r.read_offset(&mut buf, ARMED);
        assert_eq!(at, 0);
        buf[0..2].copy_from_slice(&4u16.to_le_bytes());
        r.filled(6);
        assert_eq!(r.take(&buf).expect("well formed"), Some((2, 4)));

        // A partial record of `PREFIX_LEN + CEILING - 1` bytes: the worst case.
        let partial = PREFIX_LEN + CEILING - 1;
        let at = r.read_offset(&mut buf, partial);
        buf[at..at + 2].copy_from_slice(&(CEILING as u16).to_le_bytes());
        r.filled(partial);
        assert_eq!(r.take(&buf).expect("well formed"), None, "one byte short of whole");

        let at = r.read_offset(&mut buf, ARMED);
        assert_eq!(at, partial, "the partial record moved to the front");
        assert!(buf.len() - at >= ARMED, "and the tail still takes a whole armed read");
    }

    /// A reset drops everything buffered, which is what §5.2 requires of a framing fault before
    /// teardown is reported — a peer that has lost a record boundary cannot be re-synchronised by
    /// guessing where the next one starts.
    #[test]
    fn a_reset_drops_a_partial_record() {
        let mut buf = vec![0u8; buffer_len(64, 16)];
        let mut r = Reassembler::new(64);
        buf[0..2].copy_from_slice(&40u16.to_le_bytes());
        r.filled(10);
        assert_eq!(r.buffered(), 10);
        r.reset();
        assert_eq!(r.buffered(), 0);
        assert_eq!(r.take(&buf), Ok(None));
    }
}

#[cfg(test)]
mod binding_boundaries {
    extern crate std;
    use std::vec;

    use super::*;
    use crate::flat::wire::{StreamAssembly, StreamRecordAssembler, STREAM_HEADER_LEN};

    /// **The two bindings frame differently, and one assembler cannot serve both.**
    ///
    /// This exists because the obvious economy — "USB reassembles records, so route BLE through the
    /// same code" — is wrong, and wrong in a way that would silently re-break a path the phone found
    /// on glass:
    ///
    /// * **USB** (§5.2) prefixes every record with `record_length u16`. The framing is the
    ///   binding's, sits *outside* the frame, and is what [`Reassembler`] reads.
    /// * **BLE** (§5.1) prefixes nothing. The CoC carries §3.8 records back to back and the record's
    ///   own 16-byte header is the only length there is, which is what
    ///   [`StreamRecordAssembler`] reads.
    ///
    /// So the same bytes mean different things to the two, and this pins that: a §3.8 record fed to
    /// the USB reassembler is read as a length prefix that is really the transfer's `RequestId`.
    #[test]
    fn a_ble_record_is_not_a_usb_record_and_the_assemblers_are_not_interchangeable() {
        // One §3.8 stream record: RequestId 1, offset 0, 4 payload bytes. No length prefix.
        let mut record = vec![0u8; STREAM_HEADER_LEN + 4];
        record[0..4].copy_from_slice(&1u32.to_le_bytes());
        record[12..14].copy_from_slice(&4u16.to_le_bytes());
        record[STREAM_HEADER_LEN..].copy_from_slice(&[0xAA; 4]);

        // BLE's assembler recovers it whole, from the header alone.
        let mut ble = StreamRecordAssembler::new();
        let mut into = vec![0u8; 256];
        let (used, state) = ble.push(&mut into, &record);
        assert_eq!((used, state), (record.len(), StreamAssembly::Complete(record.len())));

        // USB's reads the first two bytes as a `record_length`, which here are the low half of the
        // `RequestId` — a different number entirely. It is not a refusal, which is the point: it
        // would quietly mis-frame rather than fail, so the mistake is unrecoverable at runtime.
        let mut usb = Reassembler::new(4_112);
        let mut buf = vec![0u8; buffer_len(4_112, 64)];
        buf[..record.len()].copy_from_slice(&record);
        usb.filled(record.len());
        let declared = u16::from_le_bytes([record[0], record[1]]) as usize;
        assert_eq!(declared, 1, "the RequestId's low half read as a length");
        assert_eq!(usb.take(&buf), Ok(Some((PREFIX_LEN, 1))), "one byte, not a 20-byte record");
    }

    /// The case the phone actually hit: a record split across two CoC writes. `Reassembler` cannot
    /// express it — there is no prefix to have been split — so BLE keeps its own assembler.
    #[test]
    fn a_ble_record_split_across_writes_is_rejoined_by_its_header() {
        let mut record = [0u8; STREAM_HEADER_LEN + 8];
        record[0..4].copy_from_slice(&7u32.to_le_bytes());
        record[12..14].copy_from_slice(&8u16.to_le_bytes());
        for (i, b) in record[STREAM_HEADER_LEN..].iter_mut().enumerate() {
            *b = i as u8;
        }
        for split in 1..record.len() {
            let mut ble = StreamRecordAssembler::new();
            let mut into = vec![0u8; 256];
            let (_, first) = ble.push(&mut into, &record[..split]);
            let (_, second) = ble.push(&mut into, &record[split..]);
            assert_eq!(first, StreamAssembly::NeedMore, "split at {split} completed early");
            assert_eq!(second, StreamAssembly::Complete(record.len()), "split at {split} never completed");
            assert_eq!(&into[..record.len()], &record[..], "split at {split} rejoined wrong");
        }
    }
}
