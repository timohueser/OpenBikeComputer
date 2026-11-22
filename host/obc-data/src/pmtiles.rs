//! Reads tiles from PMTiles v3 archives on demand.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The tile id of a tile: the tiles of lower zooms, then its place on the Hilbert curve.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut id = ((1u64 << (2 * z)) - 1) / 3;
    for a in (0..z).rev() {
        let s = 1u64 << a;
        let (rx, ry) = (s & x, s & y);
        id += ((3 * rx) ^ ry) << a;
        if ry == 0 {
            if rx != 0 {
                x = (s - 1).wrapping_sub(x);
                y = (s - 1).wrapping_sub(y);
            }
            std::mem::swap(&mut x, &mut y);
        }
    }
    id
}

/// The zoom of a tile id.
pub fn zoom(id: u64) -> u8 {
    ((u64::BITS - (3 * id + 1).leading_zeros() - 1) / 2) as u8
}

/// An entry of a directory: a run of `run_length` equal tiles from `tile_id`, or with `run_length`
/// 0 the leaf directory of the tiles from `tile_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u64,
    pub run_length: u64,
}

/// A PMTiles v3 archive. It reads the root directory when it opens, and a leaf directory and a
/// tile at each read.
pub struct Archive {
    file: File,
    gzip: bool,
    tile_compression: u8,
    metadata: [u64; 2],
    leaves: u64,
    data: u64,
    root: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut header = [0u8; 127];
        read_at(&file, &mut header, 0).map_err(|e| e.to_string())?;
        if &header[..7] != b"PMTiles" || header[7] != 3 {
            return Err("not a PMTiles v3 archive".into());
        }
        let word = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
        let gzip = match header[97] {
            1 => false,
            2 => true,
            other => return Err(format!("directory compression {other}")),
        };
        let mut archive = Self {
            file,
            gzip,
            tile_compression: header[98],
            metadata: [word(24), word(32)],
            leaves: word(40),
            data: word(56),
            root: Vec::new(),
        };
        archive.root = archive.directory(word(8), word(16))?;
        Ok(archive)
    }

    /// `tile_compression` of the header: 1 none, 2 gzip.
    pub fn tile_compression(&self) -> u8 {
        self.tile_compression
    }

    /// The JSON metadata.
    pub fn metadata(&self) -> Result<serde_json::Value, String> {
        let [offset, length] = self.metadata;
        let mut raw = self.bytes(offset, length)?;
        if self.gzip {
            raw = gunzip(&raw)?;
        }
        serde_json::from_slice(&raw).map_err(|e| format!("the metadata: {e}"))
    }

    pub fn root(&self) -> &[Entry] {
        &self.root
    }

    /// `length` bytes of the file from `offset`.
    pub fn bytes(&self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        let mut out = vec![0; length as usize];
        read_at(&self.file, &mut out, offset).map_err(|e| e.to_string())?;
        Ok(out)
    }

    fn directory(&self, offset: u64, length: u64) -> Result<Vec<Entry>, String> {
        let mut raw = self.bytes(offset, length)?;
        if self.gzip {
            raw = gunzip(&raw)?;
        }
        let mut at = 0;
        let mut varint = || -> Result<u64, String> {
            let mut value = 0u64;
            for shift in (0..64).step_by(7) {
                let byte = *raw.get(at).ok_or("a directory ends inside a varint")?;
                at += 1;
                value |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return Ok(value);
                }
            }
            Err("a varint is too long".into())
        };
        let count = varint()? as usize;
        let mut entries: Vec<Entry> = Vec::with_capacity(count);
        let mut last = 0;
        for _ in 0..count {
            last += varint()?;
            entries.push(Entry { tile_id: last, offset: 0, length: 0, run_length: 0 });
        }
        for entry in &mut entries {
            entry.run_length = varint()?;
        }
        for entry in &mut entries {
            entry.length = varint()?;
        }
        for i in 0..count {
            let value = varint()?;
            entries[i].offset =
                if i > 0 && value == 0 { entries[i - 1].offset + entries[i - 1].length } else { value - 1 };
        }
        Ok(entries)
    }

    /// The bytes of a tile as the archive stores them, None where it has none.
    pub fn get(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
        self.get_id(tile_id(z, x, y))
    }

    /// The bytes of a tile without its `tile_compression`, None where the archive has none.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
        let Some(bytes) = self.get(z, x, y)? else { return Ok(None) };
        match self.tile_compression {
            1 => Ok(Some(bytes)),
            2 => gunzip(&bytes).map(Some),
            other => Err(format!("tile compression {other}")),
        }
    }

    /// The bytes of the tile of id `id` as the archive stores them, None where it has none.
    pub fn get_id(&self, id: u64) -> Result<Option<Vec<u8>>, String> {
        let mut leaf;
        let mut entries = &self.root[..];
        for _ in 0..4 {
            let Some(entry) = find(entries, id) else { return Ok(None) };
            let (offset, length) = (entry.offset, entry.length);
            if entry.run_length > 0 {
                return self.bytes(self.data + offset, length).map(Some);
            }
            leaf = self.directory(self.leaves + offset, length)?;
            entries = &leaf;
        }
        Ok(None)
    }
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The entry that holds tile `id`: a run of tiles, or the leaf directory that it lies in.
fn find(entries: &[Entry], id: u64) -> Option<&Entry> {
    let at = entries.partition_point(|entry| entry.tile_id <= id);
    let entry = entries.get(at.checked_sub(1)?)?;
    (entry.tile_id == id || entry.run_length == 0 || id - entry.tile_id < entry.run_length).then_some(entry)
}

#[cfg(unix)]
fn read_at(file: &File, out: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, out, offset)
}

/// `seek_read` can read fewer bytes than asked.
#[cfg(windows)]
fn read_at(file: &File, mut out: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !out.is_empty() {
        match file.seek_read(out, offset)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                out = &mut out[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_follow_the_hilbert_curve_of_pmtiles() {
        let tiles =
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (1, 1, 1), (1, 1, 0), (2, 3, 0), (13, 4260, 2880), (12, 2131, 1439)];
        let ids = tiles.map(|(z, x, y)| tile_id(z, x, y));
        assert_eq!(ids, [0, 1, 2, 3, 4, 20, 79377413, 19843151]);
        assert_eq!(tiles.map(|(z, x, y)| zoom(tile_id(z, x, y))), tiles.map(|t| t.0));
    }

    fn varints(values: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        for &value in values {
            let mut value = value;
            while value >= 0x80 {
                out.push(value as u8 | 0x80);
                value >>= 7;
            }
            out.push(value as u8);
        }
        out
    }

    /// An archive with no directory compression and the metadata `{"name": name}`, written to a
    /// temporary file.
    fn write(name: &str, root: &[u8], leaf: &[u8], data: &[u8], tile_compression: u8) -> std::path::PathBuf {
        let metadata = format!("{{\"name\": \"{name}\"}}");
        let (root_at, metadata_at) = (127u64, 127 + root.len() as u64);
        let leaves_at = metadata_at + metadata.len() as u64;
        let data_at = leaves_at + leaf.len() as u64;
        let mut file = b"PMTiles\x03".to_vec();
        let lengths = [root.len(), metadata.len(), leaf.len(), data.len()].map(|length| length as u64);
        let words = [root_at, lengths[0], metadata_at, lengths[1], leaves_at, lengths[2], data_at, lengths[3]];
        words.iter().for_each(|word| file.extend(word.to_le_bytes()));
        file.resize(97, 0);
        file.extend([1, tile_compression, 0]);
        file.resize(127, 0);
        file.extend(root);
        file.extend(metadata.as_bytes());
        file.extend(leaf);
        file.extend(data);
        let path = std::env::temp_dir().join(format!("obc-data-pmtiles-{name}-{}", std::process::id()));
        std::fs::write(&path, file).unwrap();
        path
    }

    #[test]
    fn an_archive_reads_a_tile_and_its_leaf_directory() {
        // Tiles 5 and 6, then a run of two equal tiles 9 and 10; the data holds "five", "six", "nine".
        // Each directory lists the count, the id deltas, the run lengths, the lengths and the offsets
        // (0 for an entry that follows the one before it, else offset + 1).
        let leaf = varints(&[3, 5, 1, 3, 1, 1, 2, 4, 3, 4, 1, 0, 0]);
        let root = varints(&[1, 5, 0, leaf.len() as u64, 1]);
        let path = write("leaf", &root, &leaf, b"fivesixnine", 1);
        let archive = Archive::open(&path).unwrap();
        assert_eq!(archive.root(), [Entry { tile_id: 5, offset: 0, length: leaf.len() as u64, run_length: 0 }]);
        let tiles: Vec<Option<Vec<u8>>> = (4..12).map(|id| archive.get_id(id).unwrap()).collect();
        let tile = |bytes: &[u8]| Some(bytes.to_vec());
        assert_eq!(tiles, [None, tile(b"five"), tile(b"six"), None, None, tile(b"nine"), tile(b"nine"), None]);
        assert_eq!(archive.get(1, 0, 0).unwrap(), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn tile_takes_off_the_gzip_of_the_archive() {
        use std::io::Write;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"snow").unwrap();
        let data = gzip.finish().unwrap();
        let root = varints(&[1, tile_id(1, 0, 1), 1, data.len() as u64, 1]);
        let path = write("gzip", &root, &[], &data, 2);
        let archive = Archive::open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(archive.get(1, 0, 1).unwrap(), Some(data));
        assert_eq!(archive.tile(1, 0, 1).unwrap(), Some(b"snow".to_vec()));
        assert_eq!(archive.tile(1, 1, 1).unwrap(), None);
        assert_eq!(archive.metadata().unwrap(), serde_json::json!({"name": "gzip"}));
    }
}
