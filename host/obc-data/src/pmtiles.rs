//! The reader of PMTiles v3 archives: a tile by its zoom and position, read from the file on
//! demand.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::FileExt;
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

/// An entry of a directory: a run of `run_length` tiles from `tile_id` with the same bytes, or,
/// with a run length of 0, a leaf directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u64,
    pub run_length: u64,
}

/// A PMTiles v3 archive, read on demand.
pub struct Archive {
    file: File,
    gzip: bool,
    /// `tile_compression` of the header: 1 none, 2 gzip.
    pub tile_compression: u8,
    /// The offset of the leaf directories.
    pub leaves: u64,
    data: u64,
    pub root: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut header = [0u8; 127];
        file.read_exact_at(&mut header, 0).map_err(|e| format!("{}: {e}", path.display()))?;
        if &header[..7] != b"PMTiles" || header[7] != 3 {
            return Err(format!("{} is not a PMTiles v3 archive", path.display()));
        }
        let word = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
        let gzip = match header[97] {
            1 => false,
            2 => true,
            other => return Err(format!("directory compression {other}")),
        };
        let mut archive =
            Self { file, gzip, tile_compression: header[98], leaves: word(40), data: word(56), root: Vec::new() };
        archive.root = archive.directory(word(8), word(16))?;
        Ok(archive)
    }

    /// `length` bytes of the file from `offset`.
    pub fn bytes(&self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        let mut out = vec![0; length as usize];
        self.file.read_exact_at(&mut out, offset).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// The entries of the directory of `length` bytes at `offset` of the file.
    pub fn directory(&self, offset: u64, length: u64) -> Result<Vec<Entry>, String> {
        let mut raw = self.bytes(offset, length)?;
        if self.gzip {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut out).map_err(|e| e.to_string())?;
            raw = out;
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
        let id = tile_id(z, x, y);
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

/// The entry that holds tile `id`: a run of tiles, or the leaf directory that it lies in.
fn find(entries: &[Entry], id: u64) -> Option<&Entry> {
    let at = entries.partition_point(|entry| entry.tile_id <= id);
    let entry = entries.get(at.checked_sub(1)?)?;
    (entry.tile_id == id || entry.run_length == 0 || id - entry.tile_id < entry.run_length).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_follow_the_hilbert_curve_of_pmtiles() {
        let tiles =
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (1, 1, 1), (1, 1, 0), (2, 3, 0), (13, 4260, 2880), (12, 2131, 1439)];
        assert_eq!(tiles.map(|(z, x, y)| tile_id(z, x, y)), [0, 1, 2, 3, 4, 20, 79377413, 19843151]);
    }
}
