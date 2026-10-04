//! Native packed storage. The query library itself only requires `Source`.
use crate::{
    blocks,
    data::Selection,
    package::{digest, Package, Source, MAX_MANIFEST_BYTES},
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

struct Pack {
    index: memmap2::Mmap,
    data: memmap2::Mmap,
    count: u64,
}

/// One pack: its digest index and its pages, both mapped, so no pack holds a file handle.
#[derive(Clone)]
pub struct Directory(Arc<Pack>);

impl Directory {
    pub fn open(path: &Path) -> Result<Package<Self>> {
        Package::open(Self::source(path)?, &manifest(&path.join("manifest.json"))?)
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
        let data = file(&path.join("pages.bin"))?;
        if data.metadata().map_err(invalid)?.len() == 0 {
            return Err(Error::InvalidData("Empty routing pack".into()));
        }
        // SAFETY: Published packages are immutable. Replacement uses another directory.
        let (index, data) =
            unsafe { (memmap2::Mmap::map(&index).map_err(invalid)?, memmap2::Mmap::map(&data).map_err(invalid)?) };
        Ok(Self(Arc::new(Pack { index, data, count })))
    }

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
                || offset.checked_add(len).is_none_or(|end| end > self.0.data.len() as u64)
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
                    if len > MAX_PAGE_BYTES as u64
                        || offset.checked_add(len).is_none_or(|end| end > self.0.data.len() as u64)
                    {
                        return Err(Error::InvalidData("Routing page outside archive".into()));
                    }
                    return Ok((offset, len));
                }
            }
        }
        Err(Error::MissingRegion(digest.into()))
    }
}

impl Source for Directory {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        let (offset, len) = self.location(digest)?;
        Ok(self.0.data[offset as usize..(offset + len) as usize].to_vec())
    }

    fn order_for_verify(&self, digests: &mut [String]) -> Result<()> {
        Files::order(digests, |digest| self.location(digest).map(|(offset, _)| (0, offset)))
    }
}

/// The packs of a grid selection. Every page is in exactly one pack.
#[derive(Clone)]
pub struct Files {
    packs: Arc<Vec<Directory>>,
    /// Digest to pack; empty while there is one pack.
    objects: Arc<Vec<([u8; 32], u32)>>,
}

impl Files {
    pub fn open(packs: Vec<Directory>) -> Result<Self> {
        let mut objects = Vec::new();
        if packs.len() > 1 {
            for (index, pack) in packs.iter().enumerate() {
                let keys = pack.digests()?;
                objects.try_reserve(keys.len()).map_err(|_| Error::Limit)?;
                objects.extend(keys.into_iter().map(|key| (key, index as u32)));
            }
            objects.sort_unstable();
            if objects.windows(2).any(|p| p[0].0 == p[1].0) {
                return Err(Error::InvalidData("Routing page occurs in more than one pack".into()));
            }
        }
        Ok(Self { packs: Arc::new(packs), objects: Arc::new(objects) })
    }

    fn pack(&self, digest: &str) -> Result<(u32, &Directory)> {
        let pack = match self.packs.len() {
            0 => return Err(Error::MissingRegion(digest.to_owned())),
            1 => 0,
            _ => {
                let wanted = key(digest)?;
                let at = self
                    .objects
                    .binary_search_by_key(&wanted, |entry| entry.0)
                    .map_err(|_| Error::MissingRegion(digest.to_owned()))?;
                self.objects[at].1
            }
        };
        Ok((pack, &self.packs[pack as usize]))
    }

    /// Sorts digests by a physical position so a verification reads each pack front to back.
    fn order(digests: &mut [String], mut position: impl FnMut(&str) -> Result<(u32, u64)>) -> Result<()> {
        let mut locations = digests
            .iter_mut()
            .map(|digest| position(digest).map(|at| (at, std::mem::take(digest))))
            .collect::<Result<Vec<_>>>()?;
        locations.sort_unstable_by_key(|(at, _)| *at);
        for (digest, (_, ordered)) in digests.iter_mut().zip(locations) {
            *digest = ordered;
        }
        Ok(())
    }
}

impl Source for Files {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        self.pack(digest)?.1.read(digest)
    }

    fn order_for_verify(&self, digests: &mut [String]) -> Result<()> {
        Self::order(digests, |digest| {
            let (index, pack) = self.pack(digest)?;
            pack.location(digest).map(|(offset, _)| (index, offset))
        })
    }
}

/// Opens a routing directory: a complete package, or a grid selection with its packs.
pub fn open(path: &Path) -> Result<Selection<Files>> {
    let blocks = path.join("blocks.json");
    if !blocks.exists() {
        let package =
            Package::open(Files::open(vec![Directory::source(path)?])?, &manifest(&path.join("manifest.json"))?)?;
        return Ok(Selection::whole(package));
    }
    let bytes = manifest(&blocks)?;
    let manifest: blocks::Manifest = serde_json::from_slice(&bytes).map_err(invalid)?;
    Selection::new(manifest, digest(&bytes), |archives| {
        Files::open(archives.iter().map(|id| Directory::source(&path.join("packs").join(id))).collect::<Result<_>>()?)
    })
}

fn manifest(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file(path)?.take(MAX_MANIFEST_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(invalid)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Limit);
    }
    Ok(bytes)
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
        let source = Directory::source(&path).unwrap();
        assert_eq!(source.read(&a).unwrap(), b"first page");
        assert_eq!(source.read(&b).unwrap(), b"second page");
        let mut keys = [b.clone(), a.clone()];
        source.order_for_verify(&mut keys).unwrap();
        assert_eq!(keys, [a.clone(), b.clone()]);
        assert!(matches!(source.order_for_verify(&mut ["0".repeat(64)]), Err(Error::MissingRegion(_))));
        assert!(matches!(source.read(&"0".repeat(64)), Err(Error::MissingRegion(_))));
        assert!(matches!(source.read("../pages.bin"), Err(Error::InvalidData(_))));
        let files = Files::open(vec![source.clone(), source.clone()]);
        assert!(matches!(files, Err(Error::InvalidData(_))));
        let files = Files::open(vec![source.clone()]).unwrap();
        assert_eq!(files.read(&b).unwrap(), b"second page");
        drop((source, files));
        let mut index = OpenOptions::new().write(true).open(path.join("pages.idx")).unwrap();
        index.seek(SeekFrom::Start(HEADER + 32)).unwrap();
        index.write_all(&u64::MAX.to_le_bytes()).unwrap();
        let first = if key(&a).unwrap() < key(&b).unwrap() { a } else { b };
        let source = Directory::source(&path).unwrap();
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
