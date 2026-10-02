//! Native packed storage. The query library itself only requires `Source`.
use crate::{
    package::{Package, Source, MAX_MANIFEST_BYTES},
    storage::MAX_PAGE_BYTES,
    table::valid_digest,
    Error, Result,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

const MAGIC: &[u8; 8] = b"OBCRIDX3";
const HEADER: u64 = 16;
const RECORD: u64 = 48;

struct Files {
    index: memmap2::Mmap,
    data: File,
    count: u64,
    bytes: u64,
}

#[derive(Clone)]
pub struct Directory(Arc<Files>);

impl Directory {
    pub fn open(path: &Path) -> Result<Package<Self>> {
        let mut manifest = Vec::new();
        file(&path.join("manifest.json"))?
            .take(MAX_MANIFEST_BYTES as u64 + 1)
            .read_to_end(&mut manifest)
            .map_err(invalid)?;
        if manifest.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        Package::open(Self::source(path)?, &manifest)
    }

    pub fn source(path: &Path) -> Result<Self> {
        let mut index = file(&path.join("pages.idx"))?;
        let mut header = [0; HEADER as usize];
        index.read_exact(&mut header).map_err(invalid)?;
        if &header[..8] != MAGIC {
            return Err(Error::InvalidData("Unsupported routing index".into()));
        }
        let count = u64::from_le_bytes(header[8..].try_into().unwrap());
        if count.checked_mul(RECORD).and_then(|n| n.checked_add(HEADER))
            != Some(index.metadata().map_err(invalid)?.len())
        {
            return Err(Error::InvalidData("Incomplete routing index".into()));
        }
        // SAFETY: Published packages are immutable. Replacement uses another directory.
        let index = unsafe { memmap2::Mmap::map(&index) }.map_err(invalid)?;
        let data = file(&path.join("pages.bin"))?;
        let bytes = data.metadata().map_err(invalid)?.len();
        Ok(Self(Arc::new(Files { index, data, count, bytes })))
    }
}

impl Source for Directory {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        let (offset, len) = self.location(digest)?;
        let mut bytes = vec![0; len as usize];
        read_at(&self.0.data, &mut bytes, offset).map_err(invalid)?;
        Ok(bytes)
    }

    fn order_for_verify(&self, digests: &mut [String]) -> Result<()> {
        let mut locations = digests
            .iter_mut()
            .map(|digest| self.location(digest).map(|(offset, _)| (offset, std::mem::take(digest))))
            .collect::<Result<Vec<_>>>()?;
        locations.sort_unstable_by_key(|(offset, _)| *offset);
        for (digest, (_, ordered)) in digests.iter_mut().zip(locations) {
            *digest = ordered;
        }
        Ok(())
    }
}

fn read_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !bytes.is_empty() {
        #[cfg(unix)]
        let result = std::os::unix::fs::FileExt::read_at(file, bytes, offset);
        #[cfg(windows)]
        let result = std::os::windows::fs::FileExt::seek_read(file, bytes, offset);
        match result {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => {
                offset += read as u64;
                bytes = &mut bytes[read..];
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl Directory {
    /// Enumerate a pack index without loading its page payloads.
    pub fn digests(&self) -> Result<Vec<[u8; 32]>> {
        let mut keys = Vec::new();
        keys.try_reserve_exact(self.0.count as usize).map_err(|_| Error::Limit)?;
        for record in self.0.index[HEADER as usize..].as_chunks::<{ RECORD as usize }>().0 {
            let key: [u8; 32] = record[..32].try_into().unwrap();
            let offset = u64::from_le_bytes(record[32..40].try_into().unwrap());
            let len = u64::from_le_bytes(record[40..48].try_into().unwrap());
            if keys.last().is_some_and(|previous| previous >= &key)
                || len > MAX_PAGE_BYTES as u64
                || offset.checked_add(len).is_none_or(|end| end > self.0.bytes)
            {
                return Err(Error::InvalidData("Invalid routing pack index".into()));
            }
            keys.push(key);
        }
        Ok(keys)
    }

    fn location(&self, digest: &str) -> Result<(u64, u64)> {
        let wanted = key(digest)?;
        let (mut low, mut high) = (0, self.0.count);
        while low < high {
            let at = low + (high - low) / 2;
            let start = (HEADER + at * RECORD) as usize;
            let record = &self.0.index[start..start + RECORD as usize];
            match record[..32].cmp(&wanted) {
                std::cmp::Ordering::Less => low = at + 1,
                std::cmp::Ordering::Greater => high = at,
                std::cmp::Ordering::Equal => {
                    let offset = u64::from_le_bytes(record[32..40].try_into().unwrap());
                    let len = u64::from_le_bytes(record[40..48].try_into().unwrap());
                    if len > MAX_PAGE_BYTES as u64 || offset.checked_add(len).is_none_or(|end| end > self.0.bytes) {
                        return Err(Error::InvalidData("Routing page outside archive".into()));
                    }
                    return Ok((offset, len));
                }
            }
        }
        Err(Error::MissingRegion(digest.into()))
    }
}

/// Writes pages once, then publishes their sorted lookup index.
pub struct Writer {
    data: File,
    index: File,
    records: BTreeMap<[u8; 32], (u64, u64)>,
    offset: u64,
}

impl Writer {
    pub fn create(path: &Path) -> Result<Self> {
        let create = |name| OpenOptions::new().write(true).create_new(true).open(path.join(name)).map_err(invalid);
        Ok(Self { data: create("pages.bin")?, index: create("pages.idx")?, records: BTreeMap::new(), offset: 0 })
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::result::Result<String, String> {
        if bytes.len() > MAX_PAGE_BYTES {
            return Err("Routing page exceeds storage budget".into());
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        if !self.records.contains_key(&digest) {
            self.data.write_all(bytes).map_err(|e| e.to_string())?;
            self.records.insert(digest, (self.offset, bytes.len() as u64));
            self.offset = self.offset.checked_add(bytes.len() as u64).ok_or("Routing archive exceeds u64")?;
        }
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn finish(mut self) -> Result<()> {
        self.index.write_all(MAGIC).map_err(invalid)?;
        self.index.write_all(&(self.records.len() as u64).to_le_bytes()).map_err(invalid)?;
        for (key, (offset, len)) in self.records {
            self.index.write_all(&key).map_err(invalid)?;
            self.index.write_all(&offset.to_le_bytes()).map_err(invalid)?;
            self.index.write_all(&len.to_le_bytes()).map_err(invalid)?;
        }
        self.index.sync_all().map_err(invalid)?;
        self.data.sync_all().map_err(invalid)
    }
}

fn key(value: &str) -> Result<[u8; 32]> {
    if !valid_digest(value) {
        return Err(Error::InvalidData("Invalid object identity".into()));
    }
    let mut key = [0; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(invalid)?;
    }
    Ok(key)
}

fn invalid(error: impl std::fmt::Display) -> Error {
    Error::InvalidData(error.to_string())
}

fn file(path: &Path) -> Result<File> {
    File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::MissingRegion(path.display().to_string())
        } else {
            invalid(e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn packed_pages_deduplicate_and_reject_invalid_ranges() {
        let path = std::env::temp_dir().join(format!("route-packed-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        let mut writer = Writer::create(&path).unwrap();
        let a = writer.write(b"first page").unwrap();
        let b = writer.write(b"second page").unwrap();
        assert_eq!(writer.write(b"first page").unwrap(), a);
        writer.finish().unwrap();
        assert_eq!(file(&path.join("pages.idx")).unwrap().metadata().unwrap().len(), HEADER + 2 * RECORD);
        let open = || {
            Directory(Arc::new(Files {
                // SAFETY: The test drops the source before it changes the index.
                index: unsafe { memmap2::Mmap::map(&file(&path.join("pages.idx")).unwrap()) }.unwrap(),
                data: file(&path.join("pages.bin")).unwrap(),
                count: 2,
                bytes: 21,
            }))
        };
        let source = open();
        assert_eq!(source.read(&a).unwrap(), b"first page");
        assert_eq!(source.read(&b).unwrap(), b"second page");
        let mut keys = [b.clone(), a.clone()];
        source.order_for_verify(&mut keys).unwrap();
        assert_eq!(keys, [a.clone(), b.clone()]);
        assert!(matches!(source.order_for_verify(&mut ["0".repeat(64)]), Err(Error::MissingRegion(_))));
        assert!(matches!(source.read(&"0".repeat(64)), Err(Error::MissingRegion(_))));
        assert!(matches!(source.read("../pages.bin"), Err(Error::InvalidData(_))));
        drop(source);
        let mut index = OpenOptions::new().write(true).open(path.join("pages.idx")).unwrap();
        index.seek(SeekFrom::Start(HEADER + 32)).unwrap();
        index.write_all(&u64::MAX.to_le_bytes()).unwrap();
        let first = if key(&a).unwrap() < key(&b).unwrap() { a } else { b };
        let source = open();
        assert!(matches!(source.read(&first), Err(Error::InvalidData(_))));
        assert!(matches!(source.order_for_verify(&mut [first]), Err(Error::InvalidData(_))));
        drop(source);
        std::fs::write(path.join("manifest.json"), b"{}").unwrap();
        index.set_len(HEADER + 1).unwrap();
        assert!(
            matches!(Directory::open(&path), Err(Error::InvalidData(message)) if message == "Incomplete routing index")
        );
    }
}
