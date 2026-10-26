//! PMTiles v3 archives: the reader of the terrain grid and the writer of the feature tiles. The
//! writer lays an archive out as the Python `pmtiles` writer does.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

/// The header fields of an archive that its writer does not derive from the tiles.
pub struct Header {
    /// `tile_type`: 0 unknown, 1 MVT, 2 PNG, 3 JPEG, 4 WebP.
    pub tile_type: u8,
    /// `tile_compression`: 1 none, 2 gzip.
    pub tile_compression: u8,
    /// West, south, east, north in degrees × 10⁷.
    pub bounds_e7: [i32; 4],
    pub center_zoom: u8,
    /// Longitude and latitude × 10⁷.
    pub center_e7: [i32; 2],
}

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
fn zoom(id: u64) -> u8 {
    ((u64::BITS - (3 * id + 1).leading_zeros() - 1) / 2) as u8
}

/// gzip of `data` at level 9, with no file name and no time, as `zlib.compress(data, 9, wbits=31)`
/// heads it.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 3];
    let mut deflate = flate2::write::DeflateEncoder::new(out, flate2::Compression::new(9));
    deflate.write_all(data).expect("deflate into memory");
    out = deflate.finish().expect("deflate into memory");
    let mut crc = flate2::Crc::new();
    crc.update(data);
    out.extend(crc.sum().to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u64,
    pub run_length: u64,
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// The gzip'd directory of the entries.
fn directory(entries: &[Entry]) -> Vec<u8> {
    let mut raw = Vec::new();
    varint(&mut raw, entries.len() as u64);
    let mut last = 0;
    for entry in entries {
        varint(&mut raw, entry.tile_id - last);
        last = entry.tile_id;
    }
    entries.iter().for_each(|entry| varint(&mut raw, entry.run_length));
    entries.iter().for_each(|entry| varint(&mut raw, entry.length));
    for (i, entry) in entries.iter().enumerate() {
        let follows = i > 0 && entry.offset == entries[i - 1].offset + entries[i - 1].length;
        varint(&mut raw, if follows { 0 } else { entry.offset + 1 });
    }
    gzip(&raw)
}

/// The root directory and the leaf directories: the entries in the root while it fits in `room`,
/// else in leaves of 4096 entries, twice as many until the root of the leaves fits.
fn directories(entries: &[Entry], room: usize) -> (Vec<u8>, Vec<u8>) {
    let root = directory(entries);
    if root.len() < room {
        return (root, Vec::new());
    }
    let mut size = 4096;
    loop {
        let (mut roots, mut leaves) = (Vec::new(), Vec::new());
        for chunk in entries.chunks(size) {
            let leaf = directory(chunk);
            roots.push(Entry {
                tile_id: chunk[0].tile_id,
                offset: leaves.len() as u64,
                length: leaf.len() as u64,
                run_length: 0,
            });
            leaves.extend(leaf);
        }
        let root = directory(&roots);
        if root.len() < room {
            return (root, leaves);
        }
        size *= 2;
    }
}

/// Write the archive of `tiles` (tile id, bytes) with the JSON `metadata`; the bytes of the archive.
/// Equal tiles share their bytes, and a run of them that follow one another shares one entry.
pub fn write(path: &Path, tiles: &[(u64, Vec<u8>)], header: &Header, metadata: &str) -> Result<u64, String> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut offsets: HashMap<&[u8], u64> = HashMap::new();
    let mut contents: Vec<&[u8]> = Vec::new();
    let (mut length, mut clustered) = (0u64, true);
    for (id, data) in tiles {
        if entries.last().is_some_and(|last| *id < last.tile_id) {
            clustered = false;
        }
        match offsets.get(&data[..]) {
            Some(&found) => {
                let last = entries.last_mut().expect("a tile before an equal one");
                if *id == last.tile_id + last.run_length && last.offset == found {
                    last.run_length += 1;
                } else {
                    entries.push(Entry { tile_id: *id, offset: found, length: data.len() as u64, run_length: 1 });
                }
            }
            None => {
                entries.push(Entry { tile_id: *id, offset: length, length: data.len() as u64, run_length: 1 });
                offsets.insert(data, length);
                contents.push(data);
                length += data.len() as u64;
            }
        }
    }
    if entries.is_empty() {
        return Err("an archive needs a tile".into());
    }
    let count = entries.len();
    entries.sort_by_key(|entry| entry.tile_id);
    let (root, leaves) = directories(&entries, 16384 - 127);
    let metadata = gzip(metadata.as_bytes());
    let mut head = Vec::with_capacity(127);
    head.extend(b"PMTiles\x03");
    let root_offset = 127u64;
    let metadata_offset = root_offset + root.len() as u64;
    let leaves_offset = metadata_offset + metadata.len() as u64;
    let data_offset = leaves_offset + leaves.len() as u64;
    let words = [
        root_offset,
        root.len() as u64,
        metadata_offset,
        metadata.len() as u64,
        leaves_offset,
        leaves.len() as u64,
        data_offset,
        length,
        tiles.len() as u64,
        count as u64,
        contents.len() as u64,
    ];
    words.iter().for_each(|word| head.extend(word.to_le_bytes()));
    head.extend([u8::from(clustered), 2, header.tile_compression, header.tile_type]);
    head.extend([zoom(entries[0].tile_id), zoom(entries[count - 1].tile_id)]);
    header.bounds_e7.iter().for_each(|value| head.extend(value.to_le_bytes()));
    head.push(header.center_zoom);
    header.center_e7.iter().for_each(|value| head.extend(value.to_le_bytes()));
    let mut file = std::io::BufWriter::new(File::create(path).map_err(|e| format!("{}: {e}", path.display()))?);
    let mut put = |bytes: &[u8]| file.write_all(bytes).map_err(|e| format!("{}: {e}", path.display()));
    for part in [&head[..], &root, &metadata, &leaves] {
        put(part)?;
    }
    for data in contents {
        put(data)?;
    }
    file.flush().map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(data_offset + length)
}

/// A PMTiles v3 archive, read on demand.
pub(crate) struct Archive {
    file: File,
    gzip: bool,
    pub(crate) tile_compression: u8,
    leaves: u64,
    data: u64,
    pub(crate) root: Vec<Entry>,
}

impl Archive {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut header = [0u8; 127];
        file.read_exact_at(&mut header, 0).map_err(|e| e.to_string())?;
        if &header[..7] != b"PMTiles" || header[7] != 3 {
            return Err("not a PMTiles v3 archive".into());
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

    pub(crate) fn bytes(&self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        let mut out = vec![0; length as usize];
        self.file.read_exact_at(&mut out, offset).map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub(crate) fn directory(&self, offset: u64, length: u64) -> Result<Vec<Entry>, String> {
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
    pub(crate) fn get(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
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
        let ids = tiles.map(|(z, x, y)| tile_id(z, x, y));
        assert_eq!(ids, [0, 1, 2, 3, 4, 20, 79377413, 19843151]);
        assert_eq!(tiles.map(|(z, x, y)| zoom(tile_id(z, x, y))), tiles.map(|t| t.0));
    }

    #[test]
    fn an_archive_reads_back_its_tiles_with_equal_ones_shared_and_runs_in_one_entry() {
        let dir = std::env::temp_dir().join(format!("obc-trailview-pmtiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("archive.pmtiles");
        let (a, b) = (gzip(b"tile a"), gzip(b"tile b"));
        // Five tiles of zoom 2: a run of two equal tiles, another, and the first content again apart.
        let ids: Vec<u64> = (5..10).collect();
        let tiles: Vec<(u64, Vec<u8>)> =
            ids.iter().zip([&a, &a, &b, &b, &a]).map(|(&id, data)| (id, data.clone())).collect();
        let header =
            Header { tile_type: 0, tile_compression: 2, bounds_e7: [1, 2, 3, 4], center_zoom: 2, center_e7: [5, 6] };
        let size = write(&path, &tiles, &header, r#"{"name": "test"}"#).unwrap();
        assert_eq!(size, std::fs::metadata(&path).unwrap().len());
        let archive = Archive::open(&path).unwrap();
        let entry =
            |tile_id, offset, length: usize, run_length| Entry { tile_id, offset, length: length as u64, run_length };
        let (al, bl) = (a.len(), b.len());
        assert_eq!(archive.root, [entry(5, 0, al, 2), entry(7, al as u64, bl, 2), entry(9, 0, al, 1)]);
        let unzip = |data: Vec<u8>| {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&data[..]).read_to_end(&mut out).unwrap();
            out
        };
        let read = |id: u64| {
            let (x, y) = (0..16).map(|k| (k % 4, k / 4)).find(|&(x, y)| tile_id(2, x, y) == id).unwrap();
            unzip(archive.get(2, x, y).unwrap().unwrap())
        };
        assert_eq!(
            ids.iter().map(|&id| read(id)).collect::<Vec<_>>(),
            [&b"tile a"[..], b"tile a", b"tile b", b"tile b", b"tile a"]
        );
        let head = archive.bytes(0, 127).unwrap();
        let word = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap());
        assert_eq!((word(72), word(80), word(88), head[96], head[100], head[101]), (5, 3, 2, 1, 2, 2));
        assert_eq!(unzip(archive.bytes(word(24), word(32)).unwrap()), br#"{"name": "test"}"#);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_root_that_does_not_fit_points_to_leaves() {
        // Tiles of irregular ids and sizes, too many entries for a root of 16 KB.
        let mut state = 1u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state >> 33
        };
        let mut id = 5_592_405;
        let tiles: Vec<(u64, Vec<u8>)> = (0..30_000u64)
            .map(|k| {
                id += 1 + next() % 500;
                let mut data = k.to_le_bytes().to_vec();
                data.resize(8 + (next() % 300) as usize, 7);
                (id, data)
            })
            .collect();
        let mut offset = 0;
        let entries: Vec<Entry> = tiles
            .iter()
            .map(|(id, data)| {
                offset += data.len() as u64;
                Entry { tile_id: *id, offset: offset - data.len() as u64, length: data.len() as u64, run_length: 1 }
            })
            .collect();
        let (root, leaves) = directories(&entries, 16384 - 127);
        assert!(root.len() < 16384 - 127 && !leaves.is_empty());
        let dir = std::env::temp_dir().join(format!("obc-trailview-leaves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("archive.pmtiles");
        let header =
            Header { tile_type: 0, tile_compression: 1, bounds_e7: [0; 4], center_zoom: 12, center_e7: [0; 2] };
        write(&path, &tiles, &header, "{}").unwrap();
        let archive = Archive::open(&path).unwrap();
        assert!(archive.root.iter().all(|entry| entry.run_length == 0));
        let leaf = archive.directory(archive.leaves + archive.root[1].offset, archive.root[1].length).unwrap();
        assert_eq!(leaf[0], entries[4096]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
