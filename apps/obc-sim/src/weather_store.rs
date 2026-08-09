//! Host filesystem adapter for the production OBCW reader/slot selector.
//!
//! WX14 will attach transfers and controls. WX7 deliberately adds only the truthful file seam so
//! simulator tests and firmware boot already make byte-for-byte identical A/B decisions.

#![allow(dead_code)]

use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use obc_formats::io::{ByteSource, Error as SourceError};
use obc_weather::{select_slots, validate_slot, Candidate, Slot, SlotSelection, SlotValidation};

pub struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let raw_len = file.metadata()?.len();
        let len = u32::try_from(raw_len)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "OBCW file exceeds uint32 length"))?;
        Ok(Self { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), SourceError> {
        let end = offset
            .checked_add(u32::try_from(out.len()).map_err(|_| SourceError::BadOffset)?)
            .ok_or(SourceError::BadOffset)?;
        if end > self.len {
            return Err(SourceError::BadOffset);
        }
        let mut file = self.file.try_borrow_mut().map_err(|_| SourceError::Io)?;
        file.seek(SeekFrom::Start(offset as u64)).map_err(|_| SourceError::Io)?;
        file.read_exact(out).map_err(|_| SourceError::Io)
    }

    fn len(&self) -> u32 {
        self.len
    }
}

pub fn inspect_root(root: &Path) -> SlotSelection {
    select_slots(inspect(root, Slot::A), inspect(root, Slot::B))
}

pub fn open_active(root: &Path, selection: SlotSelection) -> std::io::Result<Option<(Candidate, FileSource)>> {
    let Some(expected) = selection.active else { return Ok(None) };
    let source = FileSource::open(&root.join(expected.slot.root_file_name()))?;
    match validate_slot(expected.slot, &source) {
        SlotValidation::Valid(actual) if actual == expected => Ok(Some((actual, source))),
        _ => Ok(None),
    }
}

fn inspect(root: &Path, slot: Slot) -> SlotValidation {
    match FileSource::open(&root.join(slot.root_file_name())) {
        Ok(source) => validate_slot(slot, &source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SlotValidation::Missing,
        Err(_) => SlotValidation::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const A: &[u8] = include_bytes!("../../../specs/vectors/weather-minimal-dry.obcw");
    const B: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("obc-wx7-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_adapter_matches_the_shared_slice_selector() {
        let root = TempRoot::new("parity");
        fs::write(root.0.join(Slot::A.root_file_name()), A).unwrap();
        fs::write(root.0.join(Slot::B.root_file_name()), B).unwrap();
        let file_selection = inspect_root(&root.0);
        let slice_selection = select_slots(
            validate_slot(Slot::A, &obc_formats::io::SliceSource(A)),
            validate_slot(Slot::B, &obc_formats::io::SliceSource(B)),
        );
        assert_eq!(file_selection, slice_selection);
        let (candidate, source) = open_active(&root.0, file_selection).unwrap().unwrap();
        assert_eq!(validate_slot(candidate.slot, &source), SlotValidation::Valid(candidate));
    }

    #[test]
    fn corrupt_or_missing_files_fail_closed_without_whole_file_allocation() {
        let root = TempRoot::new("corrupt");
        fs::write(root.0.join(Slot::A.root_file_name()), A).unwrap();
        fs::write(root.0.join(Slot::B.root_file_name()), &B[..511]).unwrap();
        let selection = inspect_root(&root.0);
        assert_eq!(selection.active.unwrap().slot, Slot::A);
        fs::remove_file(root.0.join(Slot::A.root_file_name())).unwrap();
        assert_eq!(inspect_root(&root.0).active, None);
    }
}
