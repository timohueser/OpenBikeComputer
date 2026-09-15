//! Isolated NG2 experiment. Build with rustc; never install as an OBCM producer.
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Instant;

const CHUNK: usize = 512;
const NONE: u32 = u32::MAX;

#[derive(Default, Debug)]
struct Counts {
    reads: u64,
    read_bytes: u64,
    writes: u64,
    write_bytes: u64,
}
struct Io {
    file: File,
    counts: Counts,
}
impl Io {
    fn read(&mut self, at: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(at))?;
        self.file.read_exact(bytes)?;
        self.counts.reads += 1;
        self.counts.read_bytes += bytes.len() as u64;
        Ok(())
    }
    fn write(&mut self, at: u64, bytes: &[u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(at))?;
        self.file.write_all(bytes)?;
        self.counts.writes += 1;
        self.counts.write_bytes += bytes.len() as u64;
        Ok(())
    }
}
fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn records(chunk: &[u8; CHUNK]) -> io::Result<Vec<usize>> {
    let mut records = Vec::with_capacity(30);
    let mut at = 0;
    while at + 13 <= CHUNK && chunk[at + 12] != 255 {
        let degree = chunk[at + 12] as usize;
        if degree > 24 || at + 13 + degree * 17 > CHUNK {
            return Err(bad("invalid node record boundary"));
        }
        if records.len() == 30 {
            return Err(bad("prototype refuses more than 30 records per chunk"));
        }
        records.push(at);
        at += 13 + degree * 17;
    }
    if chunk[at..].iter().any(|&b| b != 255) {
        return Err(bad("invalid chunk padding"));
    }
    Ok(records)
}
fn lookup(table: &mut Io, id: u32, nodes: u64) -> io::Result<[u8; 12]> {
    if id as u64 >= nodes {
        return Err(bad("missing target or non-dense source id"));
    }
    let mut entry = [0; 12];
    table.read(id as u64 * 12, &mut entry)?;
    if word(&entry, 0) == NONE {
        return Err(bad("missing target"));
    }
    Ok(entry)
}
fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 4 {
        return Err(bad("usage: ng2-convert SOURCE OUTPUT SCRATCH_TABLE (new output paths)"));
    }
    let start = Instant::now();
    let mut source = Io { file: File::open(&args[1])?, counts: Counts::default() };
    let mut header = [0; 57];
    source.read(0, &mut header)?;
    if &header[..4] != b"OBCM" || header[4] != 16 || header[40] > 9 {
        return Err(bad("requires v16 OBCM"));
    }
    let unit = 1u64 << header[40];
    let mut dir = [0; 40];
    source.read(word(&header, 36) as u64 * unit, &mut dir)?;
    let count = word(&dir, 8);
    if count as u64 > 1 << 27 || u16::from_le_bytes(dir[20..22].try_into().unwrap()) != 512 {
        return Err(bad("node chunk range"));
    }
    let data = (word(&dir, 0) as u64 * unit + word(&dir, 4) as u64 * 4 + unit - 1) & !(unit - 1);
    if data + count as u64 * 512 > source.file.metadata()?.len() {
        return Err(bad("node region outside file"));
    }
    let mut chunk = [0; CHUNK];
    let mut nodes = 0u64;
    for cid in 0..count {
        source.read(data + cid as u64 * 512, &mut chunk)?;
        nodes += records(&chunk)?.len() as u64;
    }
    let mut table = Io {
        file: OpenOptions::new().read(true).write(true).create_new(true).open(&args[3])?,
        counts: Counts::default(),
    };
    let mut at = 0;
    let buffer = [255; 8192];
    while at < nodes * 12 {
        let len = ((nodes * 12 - at) as usize).min(buffer.len());
        table.write(at, &buffer[..len])?;
        at += len as u64;
    }
    for cid in 0..count {
        source.read(data + cid as u64 * 512, &mut chunk)?;
        for (ordinal, at) in records(&chunk)?.into_iter().enumerate() {
            let id = word(&chunk, at + 8);
            if id as u64 >= nodes {
                return Err(bad("source ids must be dense"));
            }
            let mut entry = [0; 12];
            table.read(id as u64 * 12, &mut entry)?;
            if word(&entry, 0) != NONE {
                return Err(bad("duplicate source id"));
            }
            entry[..4].copy_from_slice(&((cid << 5) | ordinal as u32).to_le_bytes());
            entry[4..].copy_from_slice(&chunk[at..at + 8]);
            table.write(id as u64 * 12, &entry)?;
        }
    }
    let layout_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut output = Io {
        file: OpenOptions::new().read(true).write(true).create_new(true).open(&args[2])?,
        counts: Counts::default(),
    };
    let total = source.file.metadata()?.len();
    let mut copy = [0; 8192];
    let mut at = 0;
    while at < total {
        let len = ((total - at) as usize).min(copy.len());
        source.read(at, &mut copy[..len])?;
        output.write(at, &copy[..len])?;
        at += len as u64;
    }
    let mut neighbors = 0u64;
    for cid in 0..count {
        source.read(data + cid as u64 * 512, &mut chunk)?;
        for at in records(&chunk)? {
            let node = lookup(&mut table, word(&chunk, at + 8), nodes)?;
            chunk[at + 8..at + 12].copy_from_slice(&node[..4]);
            for n in 0..chunk[at + 12] as usize {
                let nb = at + 13 + n * 17;
                let target = lookup(&mut table, word(&chunk, nb), nodes)?;
                for (coord, delta) in [(0, 4), (4, 6)] {
                    let base = word(&chunk, at + coord) as i32;
                    let diff = i16::from_le_bytes(chunk[nb + delta..nb + delta + 2].try_into().unwrap()) as i32;
                    if base.checked_add(diff) != Some(word(&target, 4 + coord) as i32) {
                        return Err(bad("neighbor coordinate mismatch"));
                    }
                }
                chunk[nb..nb + 4].copy_from_slice(&target[..4]);
                neighbors += 1;
            }
        }
        output.write(data + cid as u64 * 512, &chunk)?;
    }
    output.write(4, &[250])?;
    output.file.sync_all()?;
    let convert_ms = start.elapsed().as_secs_f64() * 1000.;
    // Independent target resolution reads the output's record boundary and coordinates.
    let mut target = [0; CHUNK];
    for cid in 0..count {
        output.read(data + cid as u64 * 512, &mut chunk)?;
        for (ordinal, at) in records(&chunk)?.into_iter().enumerate() {
            if word(&chunk, at + 8) != (cid << 5 | ordinal as u32) {
                return Err(bad("self reference mismatch"));
            }
            for n in 0..chunk[at + 12] as usize {
                let nb = at + 13 + n * 17;
                let id = word(&chunk, nb);
                if id >> 5 >= count || id & 31 >= 30 {
                    return Err(bad("target reference range"));
                }
                output.read(data + (id >> 5) as u64 * 512, &mut target)?;
                let offsets = records(&target)?;
                let pos = *offsets.get((id & 31) as usize).ok_or_else(|| bad("target ordinal missing"))?;
                if word(&target, pos + 8) != id {
                    return Err(bad("target identity mismatch"));
                }
                for (coord, delta) in [(0, 4), (4, 6)] {
                    let base = word(&chunk, at + coord) as i32;
                    let diff = i16::from_le_bytes(chunk[nb + delta..nb + delta + 2].try_into().unwrap()) as i32;
                    if base.checked_add(diff) != Some(word(&target, pos + coord) as i32) {
                        return Err(bad("resolved coordinate mismatch"));
                    }
                }
            }
        }
    }
    println!("nodes={nodes} neighbors={neighbors} chunks={count} output_bytes={total} scratch_bytes={} layout_ms={layout_ms:.3} convert_ms={convert_ms:.3} verification_ms={:.3}", nodes*12, start.elapsed().as_secs_f64()*1000.-convert_ms);
    println!("source={:?} table={:?} output={:?}", source.counts, table.counts, output.counts);
    Ok(())
}
