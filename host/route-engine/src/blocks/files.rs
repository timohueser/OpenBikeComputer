use super::{Manifest, Package};
use crate::{directory::Directory, package::Source, table::valid_digest, Error, Result};
use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Only the digest index is resident. Archive handles have a separate bounded cache.
#[derive(Clone)]
pub struct Files {
    root: PathBuf,
    objects: Arc<Vec<([u8; 32], u32)>>,
    names: Arc<Vec<String>>,
    archives: RefCell<VecDeque<(u32, Directory)>>,
}
impl Files {
    pub fn open(root: &Path) -> Result<Package<Self>> {
        let path = root.join("blocks.json");
        if std::fs::metadata(&path).map_err(|e| Error::MissingRegion(e.to_string()))?.len()
            > crate::package::MAX_MANIFEST_BYTES as u64
        {
            return Err(Error::Limit);
        }
        let bytes = std::fs::read(path).map_err(|e| Error::MissingRegion(e.to_string()))?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| Error::InvalidData(e.to_string()))?;
        if manifest.format != 2 || manifest.archives.iter().any(|id| !valid_digest(id)) {
            return Err(Error::InvalidData("Invalid routing pack directory".into()));
        }
        let mut objects = Vec::new();
        for (index, name) in manifest.archives.iter().enumerate() {
            let archive = u32::try_from(index).map_err(|_| Error::Limit)?;
            let keys = Directory::source(&root.join("packs").join(name))?.digests()?;
            objects.try_reserve(keys.len()).map_err(|_| Error::Limit)?;
            objects.extend(keys.into_iter().map(|key| (key, archive)));
        }
        objects.sort_unstable();
        if objects.windows(2).any(|p| p[0].0 == p[1].0) {
            return Err(Error::InvalidData("Routing page occurs in more than one pack".into()));
        }
        let source = Self {
            root: root.to_owned(),
            objects: Arc::new(objects),
            names: Arc::new(manifest.archives),
            archives: RefCell::new(VecDeque::new()),
        };
        Package::open(source, &bytes)
    }
}
fn key(value: &str) -> Result<[u8; 32]> {
    if !valid_digest(value) {
        return Err(Error::InvalidData("Invalid routing page digest".into()));
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|e| Error::InvalidData(e.to_string()))?;
    }
    Ok(bytes)
}
impl Source for Files {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        let wanted = key(digest)?;
        let at = self
            .objects
            .binary_search_by_key(&wanted, |entry| entry.0)
            .map_err(|_| Error::MissingRegion(digest.to_owned()))?;
        let id = self.objects[at].1;
        let mut cache = self.archives.borrow_mut();
        let archive = if let Some(at) = cache.iter().position(|(key, _)| *key == id) {
            cache.remove(at).unwrap().1
        } else {
            Directory::source(&self.root.join("packs").join(&self.names[id as usize]))?
        };
        let bytes = archive.read(digest);
        cache.push_back((id, archive));
        while cache.len() > 8 {
            cache.pop_front();
        }
        bytes
    }
    fn resident_bytes(&self) -> usize {
        self.objects.capacity() * std::mem::size_of::<([u8; 32], u32)>()
            + self.names.iter().map(|n| n.capacity() + std::mem::size_of::<String>()).sum::<usize>()
    }
}
