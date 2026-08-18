//! Pure dual-slot validation and generation selection.
//!
//! This module knows no filesystem and performs no writes. Firmware and simulator adapters hand
//! it stable [`ByteSource`]s for `WEATHER.A` and `WEATHER.B`; both therefore make the same boot
//! decision from the same bytes.

use obc_formats::io::{ByteSource, Error as SourceError};

use crate::{Error, WeatherReader};

pub const WEATHER_A_FILE: &str = "WEATHER.A";
pub const WEATHER_B_FILE: &str = "WEATHER.B";

/// The two fixed weather object slots in the card root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    pub const fn root_file_name(self) -> &'static str {
        match self {
            Self::A => WEATHER_A_FILE,
            Self::B => WEATHER_B_FILE,
        }
    }
}

/// The identity needed after full OBCW validation. No byte-layout rules are duplicated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub slot: Slot,
    pub generation: u32,
    pub generated_at: i64,
    pub total_len: u32,
    pub bundle_crc32: u32,
}

/// One slot's boot-time verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotValidation {
    Missing,
    /// The medium or filesystem could not provide a stable source.
    Unreadable,
    /// Bytes were readable but were not a complete canonical OBCW object.
    Invalid(Error),
    Valid(Candidate),
}

impl SlotValidation {
    pub const fn candidate(self) -> Option<Candidate> {
        match self {
            Self::Valid(candidate) => Some(candidate),
            _ => None,
        }
    }
}

/// Why two valid slots resolved to one active candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionReason {
    OnlyValid,
    /// RFC-1982-style serial arithmetic found an unambiguous newer generation.
    SerialNewer,
    /// Equal generations use the later producer timestamp, then stable slot A on an exact tie.
    EqualGeneration,
    /// A difference of exactly `2^31` has no serial ordering. The later producer timestamp wins,
    /// then stable slot A on an exact tie.
    HalfRangeAmbiguous,
}

/// Deterministic boot decision over both fixed slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotSelection {
    pub a: SlotValidation,
    pub b: SlotValidation,
    pub active: Option<Candidate>,
    pub reason: Option<SelectionReason>,
}

/// Fully validate one canonical slot with the shared allocation-free OBCW reader.
pub fn validate_slot<S: ByteSource + ?Sized>(slot: Slot, source: &S) -> SlotValidation {
    match WeatherReader::open(source) {
        Ok(reader) => {
            let header = reader.header();
            SlotValidation::Valid(Candidate {
                slot,
                generation: header.generation,
                generated_at: header.generated_at,
                total_len: header.total_len,
                bundle_crc32: header.crc32,
            })
        }
        Err(Error::Source(SourceError::Io)) => SlotValidation::Unreadable,
        Err(error) => SlotValidation::Invalid(error),
    }
}

/// Validate a held-magic file as its canonical byte overlay without modifying storage.
///
/// Publication writes four zero bytes at offset zero, streams bytes `4..`, closes the file, then
/// validates through this overlay. Only after it succeeds may the storage adapter patch the real
/// magic and flush the commit point.
pub fn validate_slot_with_magic<S: ByteSource + ?Sized>(slot: Slot, source: &S, magic: [u8; 4]) -> SlotValidation {
    validate_slot(slot, &MagicOverlay { source, magic })
}

/// Select the newest valid generation. Missing, unreadable and invalid slots never win.
pub fn select_slots(a: SlotValidation, b: SlotValidation) -> SlotSelection {
    let (active, reason) = match (a.candidate(), b.candidate()) {
        (None, None) => (None, None),
        (Some(candidate), None) | (None, Some(candidate)) => (Some(candidate), Some(SelectionReason::OnlyValid)),
        (Some(a_candidate), Some(b_candidate)) => {
            let (candidate, reason) = select_two(a_candidate, b_candidate);
            (Some(candidate), Some(reason))
        }
    };
    SlotSelection { a, b, active, reason }
}

/// Whether `incoming` is strictly newer than `active` under the documented serial/timestamp
/// policy. Exact ties are not replacements even if stable boot ordering would prefer slot A.
pub fn candidate_is_newer(incoming: Candidate, active: Candidate) -> bool {
    let delta = incoming.generation.wrapping_sub(active.generation);
    if delta == 0 || delta == 0x8000_0000 {
        incoming.generated_at > active.generated_at
    } else {
        delta < 0x8000_0000
    }
}

fn select_two(a: Candidate, b: Candidate) -> (Candidate, SelectionReason) {
    let delta = a.generation.wrapping_sub(b.generation);
    if delta != 0 && delta != 0x8000_0000 {
        return if delta < 0x8000_0000 { (a, SelectionReason::SerialNewer) } else { (b, SelectionReason::SerialNewer) };
    }

    let reason = if delta == 0 { SelectionReason::EqualGeneration } else { SelectionReason::HalfRangeAmbiguous };
    if a.generated_at != b.generated_at {
        return if a.generated_at > b.generated_at { (a, reason) } else { (b, reason) };
    }
    if a.slot == Slot::A {
        (a, reason)
    } else {
        (b, reason)
    }
}

struct MagicOverlay<'a, S: ByteSource + ?Sized> {
    source: &'a S,
    magic: [u8; 4],
}

impl<S: ByteSource + ?Sized> ByteSource for MagicOverlay<'_, S> {
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), SourceError> {
        self.source.read_at(offset, out)?;
        // The overlay patches the first four bytes only, so it works entirely in the low end of the
        // address space; anything past `usize` is trivially past the magic and needs no patching.
        let Ok(start) = usize::try_from(offset) else { return Ok(()) };
        let end = start.checked_add(out.len()).ok_or(SourceError::BadOffset)?;
        let overlay_start = start.min(4);
        let overlay_end = end.min(4);
        if overlay_start < overlay_end {
            let dst_start = overlay_start.checked_sub(start).ok_or(SourceError::BadOffset)?;
            out[dst_start..dst_start + (overlay_end - overlay_start)]
                .copy_from_slice(&self.magic[overlay_start..overlay_end]);
        }
        Ok(())
    }

    fn len(&self) -> u64 {
        self.source.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::SliceSource;

    fn candidate(slot: Slot, generation: u32, generated_at: i64) -> Candidate {
        Candidate { slot, generation, generated_at, total_len: 1, bundle_crc32: 2 }
    }

    #[test]
    fn serial_wrap_and_half_range_are_explicit_and_deterministic() {
        let old = candidate(Slot::A, u32::MAX - 2, 10);
        let wrapped = candidate(Slot::B, 3, 20);
        let selected = select_slots(SlotValidation::Valid(old), SlotValidation::Valid(wrapped));
        assert_eq!(selected.active, Some(wrapped));
        assert_eq!(selected.reason, Some(SelectionReason::SerialNewer));
        assert!(candidate_is_newer(wrapped, old));

        let a = candidate(Slot::A, 7, 100);
        let b = candidate(Slot::B, 7u32.wrapping_add(0x8000_0000), 101);
        let selected = select_slots(SlotValidation::Valid(a), SlotValidation::Valid(b));
        assert_eq!(selected.active, Some(b));
        assert_eq!(selected.reason, Some(SelectionReason::HalfRangeAmbiguous));
        assert!(candidate_is_newer(b, a));

        let tied_b = candidate(Slot::B, b.generation, 100);
        let selected = select_slots(SlotValidation::Valid(a), SlotValidation::Valid(tied_b));
        assert_eq!(selected.active, Some(a), "slot A is the stable exact-ambiguity tie-break");
        assert!(!candidate_is_newer(tied_b, a));
    }

    #[test]
    fn invalid_and_missing_slots_never_win() {
        let valid = candidate(Slot::B, 9, 20);
        let selected =
            select_slots(SlotValidation::Invalid(Error::Source(SourceError::Io)), SlotValidation::Valid(valid));
        assert_eq!(selected.active, Some(valid));
        assert_eq!(selected.reason, Some(SelectionReason::OnlyValid));
        assert_eq!(select_slots(SlotValidation::Missing, SlotValidation::Unreadable).active, None);
    }

    #[test]
    fn held_magic_overlay_covers_split_and_overlapping_reads() {
        let bytes = [0, 0, 0, 0, 4, 5, 6, 7];
        let source = SliceSource(&bytes);
        let overlay = MagicOverlay { source: &source, magic: *b"OBCW" };
        let mut out = [0u8; 3];
        overlay.read_at(1, &mut out).unwrap();
        assert_eq!(&out, b"BCW");
        overlay.read_at(3, &mut out).unwrap();
        assert_eq!(out, [b'W', 4, 5]);
    }
}
