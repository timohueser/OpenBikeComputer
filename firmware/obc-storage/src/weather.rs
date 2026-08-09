//! Crash-safe A/B weather publication over a storage adapter.
//!
//! The policy here never names a transport. It accepts the announced whole-object length/CRC,
//! streams canonical OBCW bytes into the inactive root slot with the first four bytes held back,
//! closes the file, validates the canonical overlay through `obc-weather`, then patches and flushes
//! the magic as the sole eligibility point. `WEATHER.A`/`WEATHER.B` are never routed through the
//! destructive route `UPLOAD.TMP` replacement path.

use obc_crc::Crc32;
use obc_formats::obcw::HEADER_LEN;
use obc_weather::{candidate_is_newer, select_slots, Candidate, Slot, SlotSelection, SlotValidation};

pub use obc_weather::{WEATHER_A_FILE, WEATHER_B_FILE};

pub const fn slot_file_name(slot: Slot) -> &'static str {
    slot.root_file_name()
}

/// Medium-specific operations. Implementations keep filesystem handles and errors out of the
/// reader/cache core while using `obc_weather::{validate_slot, validate_slot_with_magic}` inside
/// `inspect_slot` so firmware and simulator share validation policy.
pub trait WeatherSlotIo {
    type Error;

    /// Inspect a closed stable file. `magic = Some` overlays held bytes for pre-commit validation.
    fn inspect_slot(&mut self, slot: Slot, magic: Option<[u8; 4]>) -> SlotValidation;
    /// Truncate/create exactly the selected inactive slot.
    fn begin_slot(&mut self, slot: Slot) -> Result<(), Self::Error>;
    /// Append bytes to that open inactive slot.
    fn append_slot(&mut self, slot: Slot, bytes: &[u8]) -> Result<(), Self::Error>;
    /// Flush and close the inactive body before validation.
    fn close_slot(&mut self, slot: Slot) -> Result<(), Self::Error>;
    /// Best-effort close after a failed/incomplete transfer. The zero magic is deliberately kept.
    fn abandon_slot(&mut self, slot: Slot);
    /// Patch bytes `0..4`, flush, and close. An error may mean the patch persisted or did not;
    /// either outcome is recoverable because the previously active slot was never modified.
    fn commit_magic(&mut self, slot: Slot, magic: [u8; 4]) -> Result<(), Self::Error>;
}

/// Inspect both canonical root files and apply the shared deterministic selector.
pub fn inspect_slots<I: WeatherSlotIo>(io: &mut I) -> SlotSelection {
    let a = io.inspect_slot(Slot::A, None);
    let b = io.inspect_slot(Slot::B, None);
    select_slots(a, b)
}

#[derive(Debug, PartialEq, Eq)]
pub enum UploadError<E> {
    TooShort,
    Length,
    OuterCrc,
    Poisoned,
    Io(E),
    InvalidBundle(SlotValidation),
    NoSafeInactiveSlot(SlotSelection),
    NotNewer { active: Candidate, incoming: Candidate },
}

/// Successful publication. The new slot is canonical and its magic flush succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    pub installed: Candidate,
    pub previous_active: Option<Candidate>,
}

/// Small transport-independent state carried across streamed chunks.
pub struct WeatherUpload {
    target: Slot,
    active: Option<Candidate>,
    expected_len: u32,
    received: u32,
    expected_outer_crc: u32,
    running_outer_crc: Crc32,
    held_magic: [u8; 4],
    held_len: u8,
    poisoned: bool,
}

impl WeatherUpload {
    /// Select the inactive slot, truncate only it, and put a zero-magic placeholder on disk.
    pub fn begin<I: WeatherSlotIo>(
        io: &mut I,
        expected_len: u32,
        expected_outer_crc: u32,
    ) -> Result<Self, UploadError<I::Error>> {
        if expected_len < HEADER_LEN as u32 {
            return Err(UploadError::TooShort);
        }
        let selection = inspect_slots(io);
        let active = selection.active;
        let target = match active {
            Some(candidate) => candidate.slot.other(),
            None if selection.a != SlotValidation::Unreadable => Slot::A,
            None if selection.b != SlotValidation::Unreadable => Slot::B,
            None => return Err(UploadError::NoSafeInactiveSlot(selection)),
        };
        let target_validation = match target {
            Slot::A => selection.a,
            Slot::B => selection.b,
        };
        if target_validation == SlotValidation::Unreadable {
            return Err(UploadError::NoSafeInactiveSlot(selection));
        }
        io.begin_slot(target).map_err(UploadError::Io)?;
        if let Err(error) = io.append_slot(target, &[0; 4]) {
            io.abandon_slot(target);
            return Err(UploadError::Io(error));
        }
        Ok(Self {
            target,
            active,
            expected_len,
            received: 0,
            expected_outer_crc,
            running_outer_crc: Crc32::new(),
            held_magic: [0; 4],
            held_len: 0,
            poisoned: false,
        })
    }

    pub const fn target(&self) -> Slot {
        self.target
    }

    pub const fn received(&self) -> u32 {
        self.received
    }

    pub const fn remaining(&self) -> u32 {
        self.expected_len - self.received
    }

    /// Fold transport CRC and append only bytes after the held four-byte magic. A failed append
    /// poisons the transaction permanently; retry always starts from byte zero in a fresh begin.
    pub fn push<I: WeatherSlotIo>(&mut self, io: &mut I, bytes: &[u8]) -> Result<(), UploadError<I::Error>> {
        if self.poisoned {
            return Err(UploadError::Poisoned);
        }
        if bytes.len() as u64 > self.remaining() as u64 {
            self.poisoned = true;
            return Err(UploadError::Length);
        }
        self.running_outer_crc.update(bytes);
        self.received += bytes.len() as u32;

        let want = 4usize - self.held_len as usize;
        let take = want.min(bytes.len());
        self.held_magic[self.held_len as usize..self.held_len as usize + take].copy_from_slice(&bytes[..take]);
        self.held_len += take as u8;
        if take < bytes.len() {
            if let Err(error) = io.append_slot(self.target, &bytes[take..]) {
                self.poisoned = true;
                return Err(UploadError::Io(error));
            }
        }
        Ok(())
    }

    /// Validate and make the inactive generation eligible.
    pub fn finish<I: WeatherSlotIo>(self, io: &mut I) -> Result<Commit, UploadError<I::Error>> {
        if self.poisoned {
            io.abandon_slot(self.target);
            return Err(UploadError::Poisoned);
        }
        if self.received != self.expected_len || self.held_len != 4 {
            io.abandon_slot(self.target);
            return Err(UploadError::Length);
        }
        if self.running_outer_crc.finalize() != self.expected_outer_crc {
            io.abandon_slot(self.target);
            return Err(UploadError::OuterCrc);
        }
        if let Err(error) = io.close_slot(self.target) {
            io.abandon_slot(self.target);
            return Err(UploadError::Io(error));
        }

        let validation = io.inspect_slot(self.target, Some(self.held_magic));
        let incoming = match validation {
            SlotValidation::Valid(candidate) if candidate.slot == self.target => candidate,
            other => return Err(UploadError::InvalidBundle(other)),
        };
        if let Some(active) = self.active {
            if !candidate_is_newer(incoming, active) {
                return Err(UploadError::NotNewer { active, incoming });
            }
        }

        // If this returns an error after writing the magic, boot may see either valid new bytes or
        // zero magic. Both are safe: the old active slot is intact and the shared selector chooses
        // deterministically from whatever reached stable media.
        io.commit_magic(self.target, self.held_magic).map_err(UploadError::Io)?;
        Ok(Commit { installed: incoming, previous_active: self.active })
    }

    /// Orderly transport abort. It never deletes or truncates the active slot.
    pub fn abort<I: WeatherSlotIo>(self, io: &mut I) {
        io.abandon_slot(self.target);
    }
}

const _: () = assert!(core::mem::size_of::<WeatherUpload>() <= 64);

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::SliceSource;
    use obc_formats::obcw;
    use obc_weather::{validate_slot, validate_slot_with_magic, SelectionReason};
    use std::{vec, vec::Vec};

    const MINIMAL: &[u8] = include_bytes!("../../../specs/vectors/weather-minimal-dry.obcw");
    const DWD: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        CardRemoved,
        CardFull,
        Close,
        Patch,
        Flush,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Failure {
        None,
        Read(Slot),
        Begin(FakeError),
        AppendAfter(usize, FakeError),
        Close,
        Patch,
        FlushPatchLost,
        FlushPatchPersisted,
    }

    struct MemoryIo {
        slots: [Option<Vec<u8>>; 2],
        open: Option<Slot>,
        failure: Failure,
        appended: usize,
    }

    impl MemoryIo {
        fn new(a: Option<Vec<u8>>, b: Option<Vec<u8>>) -> Self {
            Self { slots: [a, b], open: None, failure: Failure::None, appended: 0 }
        }

        fn bytes(&self, slot: Slot) -> Option<&[u8]> {
            self.slots[index(slot)].as_deref()
        }

        fn power_cut(&mut self) {
            self.open = None;
        }
    }

    impl WeatherSlotIo for MemoryIo {
        type Error = FakeError;

        fn inspect_slot(&mut self, slot: Slot, magic: Option<[u8; 4]>) -> SlotValidation {
            if self.failure == Failure::Read(slot) {
                return SlotValidation::Unreadable;
            }
            let Some(bytes) = self.bytes(slot) else { return SlotValidation::Missing };
            let source = SliceSource(bytes);
            match magic {
                Some(magic) => validate_slot_with_magic(slot, &source, magic),
                None => validate_slot(slot, &source),
            }
        }

        fn begin_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
            if let Failure::Begin(error) = self.failure {
                return Err(error);
            }
            self.slots[index(slot)] = Some(Vec::new());
            self.open = Some(slot);
            self.appended = 0;
            Ok(())
        }

        fn append_slot(&mut self, slot: Slot, bytes: &[u8]) -> Result<(), Self::Error> {
            assert_eq!(self.open, Some(slot));
            if let Failure::AppendAfter(limit, error) = self.failure {
                let remaining = limit.saturating_sub(self.appended);
                let take = remaining.min(bytes.len());
                self.slots[index(slot)].as_mut().unwrap().extend_from_slice(&bytes[..take]);
                self.appended += take;
                if take < bytes.len() || self.appended >= limit {
                    return Err(error);
                }
            } else {
                self.slots[index(slot)].as_mut().unwrap().extend_from_slice(bytes);
                self.appended += bytes.len();
            }
            Ok(())
        }

        fn close_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
            assert_eq!(self.open.take(), Some(slot));
            if self.failure == Failure::Close {
                Err(FakeError::Close)
            } else {
                Ok(())
            }
        }

        fn abandon_slot(&mut self, slot: Slot) {
            if self.open == Some(slot) {
                self.open = None;
            }
        }

        fn commit_magic(&mut self, slot: Slot, magic: [u8; 4]) -> Result<(), Self::Error> {
            match self.failure {
                Failure::Patch => Err(FakeError::Patch),
                Failure::FlushPatchLost => Err(FakeError::Flush),
                Failure::FlushPatchPersisted => {
                    self.slots[index(slot)].as_mut().unwrap()[..4].copy_from_slice(&magic);
                    Err(FakeError::Flush)
                }
                _ => {
                    self.slots[index(slot)].as_mut().unwrap()[..4].copy_from_slice(&magic);
                    Ok(())
                }
            }
        }
    }

    const fn index(slot: Slot) -> usize {
        match slot {
            Slot::A => 0,
            Slot::B => 1,
        }
    }

    fn bundle(seed: &[u8], generation: u32, generated_at: i64) -> Vec<u8> {
        let mut bytes = seed.to_vec();
        bytes[obcw::HDR_GENERATION..obcw::HDR_GENERATION + 4].copy_from_slice(&generation.to_le_bytes());
        bytes[obcw::HDR_GENERATED_AT..obcw::HDR_GENERATED_AT + 8].copy_from_slice(&generated_at.to_le_bytes());
        bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
        let crc = Crc32::checksum(&bytes);
        bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn upload(io: &mut MemoryIo, bytes: &[u8]) -> Result<Commit, UploadError<FakeError>> {
        let mut upload = WeatherUpload::begin(io, bytes.len() as u32, Crc32::checksum(bytes))?;
        for chunk in bytes.chunks(173) {
            upload.push(io, chunk)?;
        }
        upload.finish(io)
    }

    #[test]
    fn successful_publish_uses_only_inactive_root_slot() {
        let old = bundle(MINIMAL, 10, 100);
        let new = bundle(DWD, 11, 200);
        let mut io = MemoryIo::new(Some(old.clone()), None);
        let commit = upload(&mut io, &new).unwrap();
        assert_eq!(commit.previous_active.unwrap().slot, Slot::A);
        assert_eq!(commit.installed.slot, Slot::B);
        assert_eq!(io.bytes(Slot::A), Some(old.as_slice()));
        assert_eq!(io.bytes(Slot::B), Some(new.as_slice()));
        let selected = inspect_slots(&mut io);
        assert_eq!(selected.active, Some(commit.installed));
    }

    #[test]
    fn partial_power_loss_at_every_block_boundary_keeps_old_bytes() {
        let old = bundle(MINIMAL, 7, 100);
        let new = bundle(DWD, 8, 200);
        let mut cuts = Vec::new();
        cuts.extend([0usize, 1, 2, 3, 4, HEADER_LEN - 1, HEADER_LEN]);
        cuts.extend((512..new.len()).step_by(512));
        cuts.push(new.len() - 1);
        cuts.sort_unstable();
        cuts.dedup();
        for cut in cuts {
            let mut io = MemoryIo::new(Some(old.clone()), None);
            {
                let mut upload = WeatherUpload::begin(&mut io, new.len() as u32, Crc32::checksum(&new)).unwrap();
                upload.push(&mut io, &new[..cut]).unwrap();
                // Abrupt reset: the state disappears without close, validation, or magic patch.
            }
            io.power_cut();
            assert_eq!(io.bytes(Slot::A), Some(old.as_slice()), "active bytes changed at cut {cut}");
            assert_eq!(inspect_slots(&mut io).active.unwrap().slot, Slot::A, "cut {cut} became eligible");
        }
    }

    #[test]
    fn outer_and_inner_crc_failures_leave_stale_valid_slot_readable() {
        let old = bundle(MINIMAL, 1, 100);
        let new = bundle(DWD, 2, 200);

        let mut outer = MemoryIo::new(Some(old.clone()), None);
        let mut transfer = WeatherUpload::begin(&mut outer, new.len() as u32, Crc32::checksum(&new) ^ 1).unwrap();
        transfer.push(&mut outer, &new).unwrap();
        assert_eq!(transfer.finish(&mut outer), Err(UploadError::OuterCrc));
        assert_eq!(outer.bytes(Slot::A), Some(old.as_slice()));
        assert_eq!(inspect_slots(&mut outer).active.unwrap().slot, Slot::A);

        let mut bad_inner = new.clone();
        let last = bad_inner.len() - 1;
        bad_inner[last] ^= 1; // outer transfer CRC matches these bytes; embedded OBCW CRC does not
        let mut inner = MemoryIo::new(Some(old.clone()), None);
        let error = upload(&mut inner, &bad_inner).unwrap_err();
        assert!(matches!(error, UploadError::InvalidBundle(SlotValidation::Invalid(_))));
        assert_eq!(inner.bytes(Slot::A), Some(old.as_slice()));
        assert_eq!(inspect_slots(&mut inner).active.unwrap().slot, Slot::A);
    }

    #[test]
    fn stale_generation_is_never_made_eligible_but_wrap_is() {
        let active = bundle(MINIMAL, 10, 200);
        let stale = bundle(DWD, 9, 300);
        let mut io = MemoryIo::new(Some(active.clone()), None);
        assert!(matches!(upload(&mut io, &stale), Err(UploadError::NotNewer { .. })));
        assert_eq!(inspect_slots(&mut io).active.unwrap().slot, Slot::A);
        assert_eq!(io.bytes(Slot::A), Some(active.as_slice()));

        let near_wrap = bundle(MINIMAL, u32::MAX - 1, 400);
        let wrapped = bundle(DWD, 2, 500);
        let mut io = MemoryIo::new(Some(near_wrap), None);
        assert_eq!(upload(&mut io, &wrapped).unwrap().installed.generation, 2);
        assert_eq!(inspect_slots(&mut io).reason, Some(SelectionReason::SerialNewer));
    }

    #[test]
    fn read_write_card_removal_and_full_fail_closed() {
        let old = bundle(MINIMAL, 1, 100);
        let new = bundle(DWD, 2, 200);
        let failures = [
            Failure::Begin(FakeError::CardRemoved),
            Failure::Begin(FakeError::CardFull),
            Failure::AppendAfter(700, FakeError::CardRemoved),
            Failure::AppendAfter(700, FakeError::CardFull),
            Failure::Close,
            Failure::Patch,
        ];
        for failure in failures {
            let mut io = MemoryIo::new(Some(old.clone()), None);
            io.failure = failure;
            let _ = upload(&mut io, &new);
            io.power_cut();
            assert_eq!(io.bytes(Slot::A), Some(old.as_slice()), "active changed for {failure:?}");
            assert_eq!(inspect_slots(&mut io).active.unwrap().slot, Slot::A, "bad selection for {failure:?}");
        }

        let mut unreadable_new = MemoryIo::new(Some(old.clone()), Some(new));
        unreadable_new.failure = Failure::Read(Slot::B);
        assert_eq!(inspect_slots(&mut unreadable_new).active.unwrap().slot, Slot::A);
        let before = unreadable_new.bytes(Slot::B).unwrap().to_vec();
        assert!(matches!(
            WeatherUpload::begin(&mut unreadable_new, DWD.len() as u32, Crc32::checksum(DWD)),
            Err(UploadError::NoSafeInactiveSlot(_))
        ));
        assert_eq!(
            unreadable_new.bytes(Slot::B),
            Some(before.as_slice()),
            "an unreadable possible active is never truncated"
        );
        let mut both_bad = MemoryIo::new(Some(vec![1, 2, 3]), Some(vec![4, 5, 6]));
        assert_eq!(inspect_slots(&mut both_bad).active, None);
    }

    #[test]
    fn failed_final_flush_recovers_whether_magic_persisted_or_not() {
        let old = bundle(MINIMAL, 4, 100);
        let new = bundle(DWD, 5, 200);

        let mut lost = MemoryIo::new(Some(old.clone()), None);
        lost.failure = Failure::FlushPatchLost;
        assert_eq!(upload(&mut lost, &new), Err(UploadError::Io(FakeError::Flush)));
        lost.power_cut();
        assert_eq!(inspect_slots(&mut lost).active.unwrap().slot, Slot::A);
        assert_eq!(lost.bytes(Slot::A), Some(old.as_slice()));

        let mut persisted = MemoryIo::new(Some(old.clone()), None);
        persisted.failure = Failure::FlushPatchPersisted;
        assert_eq!(upload(&mut persisted, &new), Err(UploadError::Io(FakeError::Flush)));
        persisted.power_cut();
        assert_eq!(inspect_slots(&mut persisted).active.unwrap().slot, Slot::B);
        assert_eq!(persisted.bytes(Slot::A), Some(old.as_slice()));
    }
}
