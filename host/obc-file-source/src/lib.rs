//! A file on disk behind [`ByteSource`], the one read seam every OBC reader is written against.
//!
//! Every host tool that opens an artefact needs the same three things: a read-only handle, the
//! length, and a read at an absolute offset. That is all this crate is, and the packer, the
//! bakery, the assembler, the inspector and the simulator now share it instead of each keeping a
//! copy.
//!
//! Format limits are *not* here. A caller that refuses a file its own container could never
//! address — `obc-pack`'s 4 GiB OBCT offset space, `obc-bake`'s 4 GB OBCM one — says so where it
//! opens the file, in its own words. The seam is `u64` and holds no opinion.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use obc_formats::io::{ByteSource, Error};

/// An open file, read at absolute offsets and never resident.
///
/// `Mutex<File>` rather than a `RefCell`: [`ByteSource::read_at`] takes `&self`, so the seek and
/// the read have to be one atomic step for the callers that share a source across threads — the
/// packer's rayon workers all sample through the same handle. The lock is held for one read, and
/// the single-threaded callers never contend for it.
pub struct FileSource {
    file: Mutex<File>,
    len: u64,
}

impl FileSource {
    /// Open `path` read-only and record its length once.
    pub fn open(path: &Path) -> std::io::Result<FileSource> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource { file: Mutex::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        let mut file = self.file.lock().map_err(|_| Error::Io)?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        file.read_exact(buf).map_err(|_| Error::Io)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file the reader can address to its last byte, and cannot address past it.
    #[test]
    fn reads_at_an_offset_and_refuses_the_end() {
        let path = obcm_testkit::scratch::scratch_path("obc-file-source", "bytes.bin");
        std::fs::write(&path, [0, 1, 2, 3, 4, 5, 6, 7]).unwrap();

        let src = FileSource::open(&path).unwrap();
        assert_eq!(src.len(), 8);

        let mut buf = [0u8; 3];
        src.read_at(5, &mut buf).unwrap();
        assert_eq!(buf, [5, 6, 7]);

        // One byte past the end, and an offset whose end overflows, are both the same refusal.
        assert_eq!(src.read_at(6, &mut buf), Err(Error::BadOffset));
        assert_eq!(src.read_at(u64::MAX, &mut buf), Err(Error::BadOffset));
    }
}
