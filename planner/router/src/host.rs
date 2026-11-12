//! Gives the route engine the bytes of the installed parts of format 10: a memory map of each file
//! on a server or a phone, or parts held in memory in WebAssembly and in tests.

use crate::{Error, Result};
use std::collections::BTreeMap;

/// One installed file: the record number of its cell in `directory.bin`, and its part number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Part {
    pub record: u32,
    pub part: u8,
}

/// A byte range of a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub part: Part,
    pub at: u32,
    pub bytes: u32,
}

/// All workers share one host.
pub trait Host: Send + Sync {
    /// The bytes of `span`. They stay valid and do not change while the host lives. A part that the
    /// install does not have is `MissingRegion`; a span that ends after its part is `InvalidData`.
    fn range(&self, span: Span) -> Result<&[u8]>;
    /// The engine reads these spans next. The call returns at once and changes no result.
    fn announce(&self, spans: &[Span]);
}

fn lend(bytes: &[u8], span: Span) -> Result<&[u8]> {
    let end = u64::from(span.at) + u64::from(span.bytes);
    bytes.get(span.at as usize..end as usize).ok_or_else(|| {
        Error::InvalidData(format!("A span of record {} part {} ends after the part", span.part.record, span.part.part))
    })
}

/// Parts in memory.
#[derive(Default)]
pub struct Memory {
    parts: BTreeMap<Part, Vec<u8>>,
}

impl Memory {
    pub fn insert(&mut self, part: Part, bytes: Vec<u8>) {
        self.parts.insert(part, bytes);
    }
}

impl Host for Memory {
    fn range(&self, span: Span) -> Result<&[u8]> {
        let part = self.parts.get(&span.part).ok_or_else(|| missing(span.part))?;
        lend(part, span)
    }

    fn announce(&self, spans: &[Span]) {
        debug_assert!(
            spans.iter().all(|span| self.range(*span).is_ok()),
            "an announced span outside its part: {spans:?}"
        );
    }
}

fn missing(part: Part) -> Error {
    Error::MissingRegion(format!("Record {} part {} is not installed", part.record, part.part))
}

#[cfg(not(target_arch = "wasm32"))]
pub use maps::Maps;

#[cfg(not(target_arch = "wasm32"))]
mod maps {
    use super::{lend, missing, Host, Part, Span};
    use crate::{cells::Directory, install, Error, Result};
    use memmap2::Mmap;
    use std::fs::File;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// Maps each installed file read-only and shared at its first use, and keeps the map until the
    /// host is dropped, also when the installer removes the file.
    pub struct Maps {
        folder: PathBuf,
        /// The level and the ID of each record.
        cells: Vec<(u8, u32)>,
        parts: usize,
        /// The size of each part of each record, from the directory.
        bytes: Vec<u32>,
        maps: Vec<OnceLock<Mmap>>,
        silent: bool,
    }

    impl Maps {
        /// The host of the install in `folder` with the directory `directory`.
        pub fn new(folder: &Path, directory: &[u8]) -> Result<Self> {
            let (installed, plan) = install::installed(folder).map_err(|e| Error::InvalidData(e.to_string()))?;
            if installed != directory {
                return Err(Error::InvalidData(format!("{} holds another directory", folder.display())));
            }
            map_limit(plan.files.len() as u64)?;
            let directory = Directory::new(directory)?;
            let parts = directory.parts();
            let (mut cells, mut bytes) = (Vec::with_capacity(directory.len()), Vec::new());
            for record in 0..directory.len() {
                let cell = directory.cell(record)?;
                cells.push((cell.level, cell.id));
                bytes.extend(cell.parts.iter().map(|part| part.bytes));
            }
            let maps = (0..bytes.len()).map(|_| OnceLock::new()).collect();
            Ok(Self { folder: folder.join(install::CELLS), cells, parts, bytes, maps, silent: false })
        }

        /// A host whose `announce` does nothing, for a measurement.
        pub fn silent(self) -> Self {
            Self { silent: true, ..self }
        }

        /// Drops each file of `folder/cells` from the page cache. The system keeps a page that a
        /// process maps, so the caller drops each host of the folder first.
        pub fn evict(folder: &Path) -> std::io::Result<()> {
            for entry in std::fs::read_dir(folder.join(install::CELLS))? {
                install::drop_cache(&File::open(entry?.path())?);
            }
            Ok(())
        }

        fn map(&self, part: Part) -> Result<&Mmap> {
            let (record, number) = (part.record as usize, usize::from(part.part));
            if record >= self.cells.len() || number >= self.parts {
                return Err(missing(part));
            }
            let index = record * self.parts + number;
            if let Some(map) = self.maps[index].get() {
                return Ok(map);
            }
            let (level, id) = self.cells[record];
            let path = self.folder.join(install::name(level, id, number));
            let failed = |e: std::io::Error| match e.kind() {
                std::io::ErrorKind::NotFound => missing(part),
                _ if refused(&e) => Error::Memory,
                _ => Error::InvalidData(format!("{}: {e}", path.display())),
            };
            let file = File::open(&path).map_err(failed)?;
            // SAFETY: the installer never changes a file under its name; it only adds and removes.
            let map = unsafe { Mmap::map(&file) }.map_err(failed)?;
            if map.len() as u64 != u64::from(self.bytes[index]) {
                return Err(Error::InvalidData(format!("{} does not have the size of its part", path.display())));
            }
            Ok(self.maps[index].get_or_init(|| map))
        }
    }

    impl Host for Maps {
        fn range(&self, span: Span) -> Result<&[u8]> {
            lend(self.map(span.part)?, span)
        }

        fn announce(&self, spans: &[Span]) {
            if self.silent {
                return;
            }
            for span in spans {
                #[cfg(unix)]
                if let Ok(map) = self.map(span.part) {
                    debug_assert!(lend(map, *span).is_ok(), "an announced span outside its part: {span:?}");
                    if lend(map, *span).is_ok() && span.bytes > 0 {
                        let _ = map.advise_range(memmap2::Advice::WillNeed, span.at as usize, span.bytes as usize);
                    }
                }
            }
        }
    }

    /// Whether the system refused a file or a map for a lack of descriptors or memory, which a
    /// retry can end: ENFILE, EMFILE and ENOMEM have these numbers on Linux, Android, macOS and iOS.
    fn refused(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::OutOfMemory || matches!(error.raw_os_error(), Some(12 | 23 | 24))
    }

    /// Fails when the installed files and the maps of the process itself do not fit the map limit
    /// of the system, so that the service does not start rather than fail a request later.
    fn map_limit(files: u64) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Ok(limit) = std::fs::read_to_string("/proc/sys/vm/max_map_count") {
            let limit: u64 = limit.trim().parse().unwrap_or(u64::MAX);
            if files + 4096 > limit {
                return Err(Error::InvalidData(format!(
                    "The install has {files} files and a process can hold {limit} memory maps: set \
                     vm.max_map_count to at least {}",
                    files + 4096
                )));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = files;
        Ok(())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::cells::{Cell, Header, Part as Entry};

    #[test]
    fn maps_and_memory_lend_the_same_bytes() {
        let folder = std::env::temp_dir().join(format!("obc-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("cells")).unwrap();
        let files: [Vec<u8>; 2] = [(0..100).collect(), (0..7).rev().collect()];
        let entry = |bytes: &Vec<u8>| Entry { bytes: bytes.len() as u32, ..Entry::default() };
        let mut directory = Vec::new();
        Header { levels: 1, download_level: 1, cells: 2, ..Header::default() }.encode(&mut directory);
        // Record 0 has two parts; record 1 has none of its own.
        let cells = [(5, vec![entry(&files[0]), entry(&files[1])]), (9, vec![Entry::default(); 2])];
        for (id, parts) in cells {
            Cell { level: 1, id, parts, ..Cell::default() }.encode(&mut directory);
        }
        for (k, bytes) in files.iter().enumerate() {
            std::fs::write(folder.join("cells").join(crate::install::name(1, 5, k)), bytes).unwrap();
        }
        std::fs::write(folder.join("directory.bin"), &directory).unwrap();
        std::fs::write(folder.join("install.bin"), crate::cells::encode_marker(&directory, 0, &[0])).unwrap();
        let maps = Maps::new(&folder, &directory).unwrap();
        let mut memory = Memory::default();
        for (k, bytes) in files.iter().enumerate() {
            memory.insert(Part { record: 0, part: k as u8 }, bytes.clone());
        }
        let hosts: [&dyn Host; 2] = [&maps, &memory];
        let span = |record, part, at, bytes| Span { part: Part { record, part }, at, bytes };
        for host in hosts {
            assert_eq!(host.range(span(0, 0, 10, 5)).unwrap(), &files[0][10..15]);
            assert_eq!(host.range(span(0, 1, 0, 7)).unwrap(), &files[1][..]);
            assert!(matches!(host.range(span(0, 1, 3, 5)), Err(Error::InvalidData(_))));
            assert!(matches!(host.range(span(1, 0, 0, 0)), Err(Error::MissingRegion(_))));
            host.announce(&[span(0, 0, 0, 100), span(0, 1, 2, 5)]);
        }
        // A map stays when its file is removed.
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(maps.range(span(0, 0, 99, 1)).unwrap(), &[99]);
    }
}
