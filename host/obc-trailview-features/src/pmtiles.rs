//! Writes the PMTiles v3 archives of the feature tiles; `obc_data::pmtiles` reads them.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use obc_data::pmtiles::{zoom, Entry};

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

#[cfg(test)]
mod tests {
    use super::*;
    use obc_data::pmtiles::{tile_id, Archive};
    use std::io::Read;

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
        assert_eq!(archive.root(), [entry(5, 0, al, 2), entry(7, al as u64, bl, 2), entry(9, 0, al, 1)]);
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
        assert!(archive.root().iter().all(|entry| entry.run_length == 0));
        assert_eq!(archive.root()[1].tile_id, entries[4096].tile_id);
        assert_eq!(archive.get_id(tiles[4096].0).unwrap().as_ref(), Some(&tiles[4096].1));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
