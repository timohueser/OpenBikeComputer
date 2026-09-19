//! The one file adapter, presenting a file to the readers exactly as the device's media does.
//!
//! A file is opened read-only and is read at an offset. That covers both seams the readers use: a
//! byte source for the container formats, and a 512-byte block device for a card image. Writes are
//! refused rather than absent, because this tool reports what is there and changes nothing.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use obc_formats::io::{ByteSource, Error};
use obc_storage::flat::BlockDevice;

pub struct FileSource {
    file: RefCell<File>,
    len: u64,
}

impl FileSource {
    pub fn open(path: &Path) -> std::io::Result<FileSource> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        file.read_exact(buf).map_err(|_| Error::Io)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// The same bytes, addressed in 512-byte blocks — what a card image is. Implemented for the
/// reference, the shape every card in this tree has: the store takes its device by value and a
/// caller keeps the file.
impl BlockDevice for &FileSource {
    type Error = Error;

    fn block_count(&self) -> Result<u64, Error> {
        Ok(self.len / 512)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), Error> {
        let offset = lba.checked_mul(512).ok_or(Error::BadOffset)?;
        ByteSource::read_at(*self, offset, buf)
    }

    fn write(&self, _lba: u64, _buf: &[u8]) -> Result<(), Error> {
        Err(Error::Io)
    }

    fn sync(&self) -> Result<(), Error> {
        Err(Error::Io)
    }
}
