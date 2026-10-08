//! Native file-backed output and owned spill storage.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::{Error, MapStore, Result, ScratchId, ScratchStore};
use obc_file_source::FileSource;
use obc_formats::io::{ByteSource, Error as IoError};

/// The map as one file at a path the caller named: the name is the caller's, and replacing a map
/// is truncating it.
pub struct FileStore {
    path: PathBuf,
    open: Option<std::io::BufWriter<File>>,
    sealed: Option<FileSource>,
}

impl FileStore {
    pub fn new(path: &Path) -> Result<FileStore> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| Error::Io(IoError::Io))?;
        }
        Ok(FileStore { path: path.to_path_buf(), open: None, sealed: None })
    }
}

impl MapStore for FileStore {
    fn begin(&mut self) -> Result<()> {
        self.sealed = None;
        let file = File::create(&self.path).map_err(|_| Error::Io(IoError::Io))?;
        self.open = Some(std::io::BufWriter::new(file));
        Ok(())
    }
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.open.as_mut().expect("the map is open").write_all(buf).map_err(|_| Error::Io(IoError::Io))
    }
    fn seal(&mut self) -> Result<()> {
        let mut w = self.open.take().expect("the map is open");
        w.flush().map_err(|_| Error::Io(IoError::Io))?;
        drop(w);
        self.sealed = Some(FileSource::open(&self.path).map_err(|_| Error::Io(IoError::Io))?);
        Ok(())
    }
    fn source(&self) -> Result<&dyn ByteSource> {
        self.sealed.as_ref().map(|s| s as &dyn ByteSource).ok_or(Error::Io(IoError::BadOffset))
    }
}

/// Spill files in one owned directory per assembly. Finished files are removed immediately;
/// dropping the store closes handles and removes the directory, including after an error.
pub struct FileScratch {
    dir: tempfile::TempDir,
    /// Open handles by [`ScratchId`], with each one's length so an append never has to seek to find
    /// the end. `None` is a removed file, so an id is never reused.
    files: RefCell<Vec<Option<(File, u64)>>>,
}

impl FileScratch {
    pub fn new() -> Result<FileScratch> {
        let dir = tempfile::Builder::new()
            .prefix("obcm-assemble-scratch-")
            .tempdir()
            .map_err(|e| Error::Scratch(format!("create scratch directory: {e}")))?;
        Ok(FileScratch { dir, files: RefCell::new(Vec::new()) })
    }

    /// Run `f` against the open handle, or refuse — never silently against the wrong file.
    fn with<T>(&self, id: ScratchId, f: impl FnOnce(&mut (File, u64)) -> Result<T>) -> Result<T> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize).and_then(Option::as_mut) {
            Some(entry) => f(entry),
            None => Err(Error::Scratch(format!("{id} is not open"))),
        }
    }
}

impl ScratchStore for FileScratch {
    fn create(&self) -> Result<ScratchId> {
        let mut files = self.files.borrow_mut();
        let id = ScratchId(u32::try_from(files.len()).map_err(|_| Error::Scratch("too many scratch files".into()))?);
        let path = self.dir.path().join(format!("{}.spill", id.0));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| Error::Scratch(format!("create {}: {e}", path.display())))?;
        files.push(Some((file, 0)));
        Ok(id)
    }

    fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()> {
        self.with(id, |(file, len)| {
            file.seek(SeekFrom::Start(*len)).map_err(|e| Error::Scratch(format!("{id}: seek: {e}")))?;
            file.write_all(buf).map_err(|e| Error::Scratch(format!("{id}: write: {e}")))?;
            *len += buf.len() as u64;
            Ok(())
        })
    }

    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.with(id, |(file, len)| {
            let end = offset.saturating_add(buf.len() as u64);
            if end > *len {
                return Err(Error::Scratch(format!(
                    "{id}: a read of {} byte(s) at {offset} runs past the {len}-byte end",
                    buf.len()
                )));
            }
            file.seek(SeekFrom::Start(offset)).map_err(|e| Error::Scratch(format!("{id}: seek: {e}")))?;
            file.read_exact(buf).map_err(|e| Error::Scratch(format!("{id}: read: {e}")))
        })
    }

    fn len(&self, id: ScratchId) -> Result<u64> {
        self.with(id, |(_, len)| Ok(*len))
    }

    fn remove(&self, id: ScratchId) -> Result<()> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize) {
            Some(slot) => {
                *slot = None; // closes the handle
                let _ = std::fs::remove_file(self.dir.path().join(format!("{}.spill", id.0)));
                Ok(())
            }
            None => Err(Error::Scratch(format!("{id} is not open"))),
        }
    }
}

impl Drop for FileScratch {
    fn drop(&mut self) {
        self.files.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStore;

    #[test]
    fn native_output_matches_memory_and_scratch_lifetimes_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let mut disk = FileStore::new(&dir.path().join("map.obcm")).unwrap();
        let mut memory = MemoryStore::default();
        disk.begin().unwrap();
        memory.begin().unwrap();
        for body in [b"one".as_slice(), b"two".as_slice()] {
            disk.write(body).unwrap();
            memory.write(body).unwrap();
        }
        disk.seal().unwrap();
        memory.seal().unwrap();
        let mut body = [0; 6];
        disk.source().unwrap().read_at(0, &mut body).unwrap();
        assert_eq!(body.as_slice(), memory.map.0);
        let first = FileScratch::new().unwrap();
        let second = FileScratch::new().unwrap();
        let first_dir = first.dir.path().to_path_buf();
        let second_dir = second.dir.path().to_path_buf();
        assert_ne!(first_dir, second_dir);
        let a = first.create().unwrap();
        let b = second.create().unwrap();
        first.append(a, b"first").unwrap();
        second.append(b, b"other").unwrap();
        drop(first);
        assert!(!first_dir.exists());
        let mut body = [0; 5];
        second.read_at(b, 0, &mut body).unwrap();
        assert_eq!(&body, b"other");
        assert!(second.read_at(b, 1, &mut body).is_err());
        second.remove(b).unwrap();
        assert!(second.len(b).is_err());
        drop(second);
        assert!(!second_dir.exists());
    }
}
