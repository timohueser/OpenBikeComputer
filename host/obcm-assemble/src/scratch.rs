//! The **scratch seam** — where the engine puts bookkeeping it is not allowed to keep in memory.
//!
//! The engine never touches a filesystem: it has to run in a browser tab, so every byte it reads
//! crosses [`obc_formats::io::ByteSource`] and every byte it writes crosses [`crate::MapStore`].
//! That rule is what this module extends rather than breaks. A country-scale merge's *bookkeeping*
//! — not its output, not its input — is measured in gigabytes at DACH scale (#1116 phase D), and
//! none of it is information that has to be resident: every global step has a sorted-pass
//! equivalent, and a sorted pass needs somewhere to spill.
//!
//! [`ScratchStore`] is that somewhere, and it is a **host** capability exactly as the shard store
//! is. The native CLI backs it with temp files; a browser backs it with OPFS sync access handles;
//! the tests and any host without storage use [`MemoryScratch`], which is honest about being a
//! fallback rather than a win.
//!
//! # The contract, and why it is shaped like this
//!
//! * **Anonymous.** A scratch file has no name a caller chooses and no meaning outside the run that
//!   created it. [`ScratchStore::create`] mints an opaque [`ScratchId`]; nothing else addresses it.
//! * **Append, then read at.** Every producer in the merge writes a stream front to back and every
//!   consumer reads ranges of it. There is no `write_at`, because nothing needs one and because a
//!   random-access write is the one operation an OPFS handle makes a host reason about
//!   (`FileSystemSyncAccessHandle.write` past the end silently zero-fills).
//! * **`&self`, not `&mut self`.** A k-way merge reads a dozen runs *while* writing the next one.
//!   With `&mut self` on the write half that is a borrow conflict at every call site, so the store
//!   carries its own interior mutability — the same choice [`obc_formats::io::ByteSource`] and the
//!   CLI's `FileSource` already made.
//! * **`u64` offsets.** OBCM addresses bytes with `uint32` because a *map file* does; a merge's
//!   spill does not, and a DACH edge stream passes 4 GiB.
//! * **Synchronous.** The assembly is one straight-line call, and in the browser it runs on a worker
//!   precisely so a blocking read is legal there. A future-shaped seam could not be called from the
//!   middle of the merge at all.
//! * **No borrows of store-owned buffers.** Every read fills the caller's slice. A
//!   `fn read(&self) -> &[u8]` would force a file-sized buffer to exist somewhere, which is the
//!   thing this seam exists to avoid.
//!
//! # Lifetime of a scratch file
//!
//! The engine deletes what it creates as soon as the last reader is done with it, and a host is
//! expected to clean the rest up when the run ends anyway (the CLI removes its whole temp directory
//! on drop). A delete that fails is **not** an assembly failure: the bytes are already unreachable
//! and the map is unaffected.

use std::cell::RefCell;

use crate::{Error, Result};

/// A handle to one anonymous scratch file. Opaque: only the store that minted it can interpret it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScratchId(pub u32);

impl core::fmt::Display for ScratchId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "scratch file #{}", self.0)
    }
}

/// Where the engine spills the passes it may not hold in memory. See the module header for the
/// contract; the short version is *anonymous files, append-only writes, `u64` random-access reads,
/// synchronous, `&self`*.
pub trait ScratchStore {
    /// Mint an empty scratch file.
    fn create(&self) -> Result<ScratchId>;
    /// Append `buf` to `id`'s current end.
    fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()>;
    /// Fill `buf` with exactly `buf.len()` bytes starting at `offset`. A short read is a failure:
    /// the buffer must be filled or the call must refuse, or a truncated spill reads as data.
    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()>;
    /// How many bytes have been appended to `id`.
    fn len(&self, id: ScratchId) -> Result<u64>;
    /// Drop `id` and everything in it. Idempotent from the engine's side: it is called once, and a
    /// host may already have reclaimed the bytes.
    fn remove(&self, id: ScratchId) -> Result<()>;
}

/// A [`ScratchStore`] that keeps the spill in memory.
///
/// It is the **fallback**, not the destination: a spill held in RAM is exactly the residency the
/// spill exists to remove. It is here because two callers legitimately want it — the test suite,
/// where a fixture's whole scratch is a few kilobytes, and a host that has no storage to offer, for
/// which a slightly worse peak beats a refusal to assemble at all.
#[derive(Default, Debug)]
pub struct MemoryScratch {
    /// `None` marks a removed file, so ids are never reused and a use-after-remove is a refusal
    /// rather than someone else's bytes.
    files: RefCell<Vec<Option<Vec<u8>>>>,
}

impl MemoryScratch {
    pub fn new() -> MemoryScratch {
        MemoryScratch::default()
    }

    /// Bytes currently held across every live file — what a test asserts a budget against.
    pub fn resident_bytes(&self) -> usize {
        self.files.borrow().iter().flatten().map(Vec::len).sum()
    }

    fn with<T>(&self, id: ScratchId, f: impl FnOnce(&mut Vec<u8>) -> Result<T>) -> Result<T> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize).and_then(Option::as_mut) {
            Some(buf) => f(buf),
            None => Err(Error::Scratch(format!("{id} does not exist"))),
        }
    }
}

impl ScratchStore for MemoryScratch {
    fn create(&self) -> Result<ScratchId> {
        let mut files = self.files.borrow_mut();
        files.push(Some(Vec::new()));
        Ok(ScratchId(u32::try_from(files.len() - 1).map_err(|_| Error::Scratch("too many scratch files".into()))?))
    }

    fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()> {
        self.with(id, |file| {
            file.extend_from_slice(buf);
            Ok(())
        })
    }

    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.with(id, |file| {
            let at = usize::try_from(offset).map_err(|_| Error::Scratch(format!("{id}: offset {offset} overflows")))?;
            let end = at.checked_add(buf.len()).ok_or_else(|| Error::Scratch(format!("{id}: read overflows")))?;
            if end > file.len() {
                return Err(Error::Scratch(format!(
                    "{id}: a read of {} byte(s) at {at} runs past the {}-byte end",
                    buf.len(),
                    file.len()
                )));
            }
            buf.copy_from_slice(&file[at..end]);
            Ok(())
        })
    }

    fn len(&self, id: ScratchId) -> Result<u64> {
        self.with(id, |file| Ok(file.len() as u64))
    }

    fn remove(&self, id: ScratchId) -> Result<()> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize) {
            Some(slot) => {
                *slot = None;
                Ok(())
            }
            None => Err(Error::Scratch(format!("{id} does not exist"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_file_reads_back_what_was_appended_to_it() {
        let s = MemoryScratch::new();
        let a = s.create().expect("a file");
        let b = s.create().expect("another file");
        assert_ne!(a, b, "ids are distinct");
        s.append(a, b"hello ").expect("append");
        s.append(a, b"world").expect("append");
        s.append(b, b"other").expect("append");
        assert_eq!(s.len(a).expect("len"), 11);

        let mut buf = [0u8; 5];
        s.read_at(a, 6, &mut buf).expect("read");
        assert_eq!(&buf, b"world", "a read at an offset is that offset's bytes");
        s.read_at(b, 0, &mut buf).expect("read");
        assert_eq!(&buf, b"other", "and the two files do not share a cursor");
    }

    #[test]
    fn a_read_past_the_end_is_refused_rather_than_short() {
        let s = MemoryScratch::new();
        let id = s.create().expect("a file");
        s.append(id, b"1234").expect("append");
        let mut buf = [0u8; 8];
        let err = s.read_at(id, 0, &mut buf).expect_err("eight bytes are not there");
        assert!(format!("{err}").contains("runs past"), "got: {err}");
        assert_eq!(buf, [0u8; 8], "and nothing was written into the caller's buffer");
    }

    #[test]
    fn a_removed_file_is_gone_and_its_id_is_never_reused() {
        let s = MemoryScratch::new();
        let a = s.create().expect("a file");
        s.append(a, &[7u8; 64]).expect("append");
        assert_eq!(s.resident_bytes(), 64);
        s.remove(a).expect("remove");
        assert_eq!(s.resident_bytes(), 0, "removing frees the bytes");
        let b = s.create().expect("another file");
        assert_ne!(a, b, "the removed id is not handed out again");
        let err = s.append(a, b"x").expect_err("the removed file is gone");
        assert!(format!("{err}").contains("does not exist"), "got: {err}");
    }
}
