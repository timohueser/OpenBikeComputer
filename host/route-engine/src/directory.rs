//! Native storage adapter. The query library itself only requires `Source`.
use crate::{
    package::{Package, Source, MAX_MANIFEST_BYTES},
    Error, Result,
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Directory(PathBuf);

impl Directory {
    pub fn open(path: &Path) -> Result<Package<Self>> {
        let manifest = read_limited(&path.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        Package::open(Self(path.join("objects")), &manifest)
    }
}

impl Source for Directory {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidData("Invalid object identity".into()));
        }
        read_limited(&self.0.join(digest), crate::storage::MAX_PAGE_BYTES)
    }
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::MissingRegion(path.display().to_string())
        } else {
            Error::InvalidData(e.to_string())
        }
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| Error::InvalidData(e.to_string()))?;
    if bytes.len() > limit {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
