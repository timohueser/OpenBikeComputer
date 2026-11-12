//! Installs a set of download cells of a route package of format 10 into a folder, one file for
//! each part, and checks the published objects and the installed files.
//!
//! The folder holds `directory.bin`, `package.json`, `cells/LEVEL-ID-PART`, `install.lock` and
//! `install.bin`. A file with its final name in `cells/` is complete and of this directory, so a
//! second call goes on after a stop. A reader trusts the files once `install.bin` names the
//! directory.

use crate::cells::{self, Directory};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

pub const LOCK: &str = "install.lock";
pub const MARKER: &str = "install.bin";
pub const CELLS: &str = "cells";
/// The read buffer of an object and the write buffer of a part.
const BUFFER: usize = 256 * 1024;
/// The largest zstd window of an object.
const WINDOW: u64 = 8 << 20;

/// Why an install or a check failed. Each case asks the caller for another action.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Another install holds the lock of the folder: try again later.
    #[error("another install holds {0}")]
    Locked(PathBuf),
    /// A published object is missing, damaged or of another package: fetch it again.
    #[error("object {object}: {reason}")]
    Object { object: String, reason: String },
    /// A file of the folder could not be read or written, for example on a full disk.
    #[error("{path}: {error}")]
    Io { path: PathBuf, error: io::Error },
    /// The directory, `package.json`, the selection or the folder does not fit the package.
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Invalid(error.to_string())
}

fn io(path: &Path) -> impl Fn(io::Error) -> Error + '_ {
    move |error| Error::Io { path: path.to_path_buf(), error }
}

/// One file of an install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The record number in the directory.
    pub record: u32,
    pub part: u8,
    /// `LEVEL-ID-PART`, the name in `cells/`.
    pub name: String,
    pub object: [u8; 32],
    pub object_bytes: u32,
    pub sha256: [u8; 32],
    pub bytes: u32,
}

impl Item {
    fn wrong(&self, reason: impl std::fmt::Display) -> Error {
        Error::Object { object: hex(&self.object), reason: reason.to_string() }
    }
}

/// The files of an install: the cells, the groups and a file for each part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Bit `g` for each installed profile group.
    pub groups: u32,
    /// The record numbers of the installed cells, ascending.
    pub records: Vec<u32>,
    pub files: Vec<Item>,
}

impl Plan {
    /// The plan without the files that `target/cells/` has with their final name.
    pub fn missing(&self, target: &Path) -> Plan {
        let cells = target.join(CELLS);
        let files = self.files.iter().filter(|item| !cells.join(&item.name).is_file()).cloned().collect();
        Plan { files, ..self.clone() }
    }
}

/// The installed bytes of the files of `plan`.
pub fn installed_bytes(plan: &Plan) -> u64 {
    plan.files.iter().map(|item| u64::from(item.bytes)).sum()
}

/// The published bytes of the objects of `plan`.
pub fn object_bytes(plan: &Plan) -> u64 {
    plan.files.iter().map(|item| u64::from(item.object_bytes)).sum()
}

/// What `install` and `check` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The files written, or checked by `check`, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// The bytes of the objects read.
    pub object_bytes: u64,
    /// The files that `install` deleted.
    pub removed: u64,
}

fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn name(cell: &cells::Cell, part: usize) -> String {
    format!("{}-{:08x}-{part}", cell.level, cell.id)
}

/// The group of each profile of `package.json`, by name.
fn groups(info: &[u8]) -> Result<BTreeMap<String, u32>> {
    let info: serde_json::Value = serde_json::from_slice(info).map_err(|e| invalid(format!("package.json: {e}")))?;
    let profiles = info["profiles"].as_array().ok_or_else(|| invalid("package.json lists no profiles"))?;
    profiles
        .iter()
        .map(|entry| {
            let group = entry["group"].as_u64().filter(|&g| g < cells::MAX_GROUPS as u64);
            match (entry["profile"]["name"].as_str(), group) {
                (Some(name), Some(group)) => Ok((name.to_string(), group as u32)),
                _ => Err(invalid("package.json has a bad profile entry")),
            }
        })
        .collect()
}

/// The files of the cells `records` with the parts of the groups of `mask`.
fn items(directory: &Directory, records: &[u32], mask: u32) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for &record in records {
        let cell = directory.cell(record as usize).map_err(invalid)?;
        for (part, entry) in cell.parts.iter().enumerate() {
            let group = part.checked_sub(2).map(|g| mask >> g & 1 != 0);
            if entry.absent() || group == Some(false) {
                continue;
            }
            items.push(Item {
                record,
                part: part as u8,
                name: name(&cell, part),
                object: entry.object,
                object_bytes: entry.object_bytes,
                sha256: entry.sha256,
                bytes: entry.bytes,
            });
        }
    }
    Ok(items)
}

/// The plan of an install of the download cells with the IDs `cells`, or all, with the groups of
/// `profiles`, or all, of the package of `directory` and `info` (`package.json`). It adds each
/// upper cell whose download cells are all in the install.
pub fn plan(directory: &[u8], info: &[u8], cells: Option<&[u32]>, profiles: Option<&[String]>) -> Result<Plan> {
    let directory = Directory::new(directory).map_err(invalid)?;
    let header = directory.header;
    let groups = groups(info)?;
    let all = if header.groups == 32 { u32::MAX } else { (1u32 << header.groups) - 1 };
    let mask = match profiles {
        None => all,
        Some(names) => names.iter().try_fold(0, |mask, name| {
            groups
                .get(name)
                .map(|&g| mask | 1 << g)
                .ok_or_else(|| invalid(format!("the package has no profile {name}")))
        })?,
    };
    let downloads = directory.downloads();
    let mut records: Vec<u32> = match cells {
        None => (0..downloads as u32).collect(),
        Some(ids) => ids
            .iter()
            .map(|&id| match directory.find(header.download_level, id).map_err(invalid)? {
                Some(record) if directory.cell(record).map_err(invalid)?.id == id => Ok(record as u32),
                _ => Err(invalid(format!("the package has no download cell {id:08x}"))),
            })
            .collect::<Result<_>>()?,
    };
    records.sort_unstable();
    records.dedup();
    // The download cells below each upper cell, and those of the install.
    let mut below: BTreeMap<(u8, u32), (u32, u32)> = BTreeMap::new();
    let mut chosen = records.iter().peekable();
    for record in 0..downloads as u32 {
        let id = directory.cell(record as usize).map_err(invalid)?.id;
        let selected = chosen.next_if(|&&r| r == record).is_some();
        for level in header.download_level + 1..=header.levels {
            let count = below.entry((level, id & !directory.below(level))).or_default();
            count.0 += 1;
            count.1 += u32::from(selected);
        }
    }
    for ((level, id), (all, selected)) in below {
        if all == selected {
            let record = directory.find(level, id).map_err(invalid)?;
            let record =
                record.ok_or_else(|| invalid(format!("the directory has no cell {id:08x} of level {level}")))?;
            records.push(record as u32);
        }
    }
    let files = items(&directory, &records, mask)?;
    Ok(Plan { groups: mask, records, files })
}

/// A reader that hashes and counts the bytes it reads.
struct Hashing<R> {
    inner: R,
    hash: Sha256,
    bytes: u64,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hash.update(&buf[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

/// Asks the kernel to drop the cached pages of `file`, where it can.
pub(crate) fn drop_cache(file: &File) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for the call; the advice changes no data.
        unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let _ = file;
}

/// The window, the content size and the dictionary flag of the zstd frame header at the start of
/// `bytes`, or `None` for bytes that do not start with one.
fn frame_header(bytes: &[u8]) -> Option<(u64, Option<u64>, bool)> {
    if bytes.get(..4)? != [0x28, 0xB5, 0x2F, 0xFD] {
        return None;
    }
    let descriptor = *bytes.get(4)?;
    let single = descriptor >> 5 & 1 == 1;
    let mut at = 5;
    let mut window = 0;
    if !single {
        let byte = *bytes.get(5)?;
        let base = 1u64 << (10 + (byte >> 3));
        window = base + base / 8 * u64::from(byte & 7);
        at += 1;
    }
    at += [0, 1, 2, 4][usize::from(descriptor & 3)];
    let size = match descriptor >> 6 {
        0 if !single => None,
        flag => {
            let width = [1, 2, 4, 8][usize::from(flag)];
            let field = bytes.get(at..at + width)?;
            let value = field.iter().rev().fold(0u64, |value, &byte| value << 8 | u64::from(byte));
            Some(if width == 2 { value + 256 } else { value })
        }
    };
    Some((if single { size? } else { window }, size, descriptor & 3 != 0))
}

/// Decompresses the object of `item` from `object` into `out`, and fails unless the object and the
/// part have their hashes and sizes. Before it decodes, it requires the content size of the part
/// and a window of at most 8 MiB. `buffer` holds the part bytes between the reads and writes.
fn stream(
    item: &Item,
    object: &File,
    decoder: &mut ruzstd::decoding::FrameDecoder,
    buffer: &mut [u8],
    mut out: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut source = Hashing { inner: io::BufReader::with_capacity(BUFFER, object), hash: Sha256::new(), bytes: 0 };
    match frame_header(source.inner.fill_buf().map_err(|e| item.wrong(e))?) {
        Some((window, Some(size), false)) if window <= WINDOW && size == u64::from(item.bytes) => {}
        _ => return Err(item.wrong("not one zstd frame of its part with a window of at most 8 MiB")),
    }
    let mut part = (Sha256::new(), 0u64);
    {
        let mut frame = ruzstd::decoding::StreamingDecoder::new_with_decoder(&mut source, &mut *decoder)
            .map_err(|e| item.wrong(format!("not a zstd frame: {e}")))?;
        loop {
            let read = frame.read(buffer).map_err(|e| item.wrong(format!("bad zstd data: {e}")))?;
            if read == 0 {
                break;
            }
            part.1 += read as u64;
            if part.1 > u64::from(item.bytes) {
                return Err(item.wrong("it holds more bytes than its part"));
            }
            part.0.update(&buffer[..read]);
            out(&buffer[..read])?;
        }
    }
    io::copy(&mut source, &mut io::sink()).map_err(|e| item.wrong(e))?;
    if <[u8; 32]>::from(source.hash.finalize()) != item.object || source.bytes != u64::from(item.object_bytes) {
        return Err(item.wrong("its bytes differ from the directory"));
    }
    if <[u8; 32]>::from(part.0.finalize()) != item.sha256 || part.1 != u64::from(item.bytes) {
        return Err(item.wrong("its part differs from the directory"));
    }
    Ok(())
}

/// Runs `work` on each item on `threads` workers, each with its own decoder and buffer. Returns the
/// error of the first item in plan order that failed; the workers stop at the first error.
fn each<F>(items: &[Item], threads: usize, work: F) -> Result<()>
where
    F: Fn(&Item, &mut ruzstd::decoding::FrameDecoder, &mut [u8]) -> Result<()> + Sync,
{
    let (next, stop, errors) = (AtomicUsize::new(0), AtomicBool::new(false), Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, items.len().max(1)) {
            scope.spawn(|| {
                let mut decoder = ruzstd::decoding::FrameDecoder::new();
                let mut buffer = vec![0; BUFFER];
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    if let Err(error) = work(item, &mut decoder, &mut buffer) {
                        stop.store(true, Ordering::Relaxed);
                        errors.lock().unwrap_or_else(|e| e.into_inner()).push((index, error));
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap_or_else(|e| e.into_inner());
    errors.into_iter().min_by_key(|(index, _)| *index).map_or(Ok(()), |(_, error)| Err(error))
}

/// Writes `bytes` to `path` through a temporary file, a sync and a rename.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary).map_err(io(&temporary))?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(io(&temporary))?;
    fs::rename(&temporary, path).map_err(io(path))
}

/// Keeps a file with the same bytes; writes a missing one. Fails on a file with other bytes.
fn keep(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::read(path) {
        Ok(found) if found == bytes => Ok(()),
        Ok(_) => Err(invalid(format!("{} holds another package", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => replace(path, bytes),
        Err(e) => Err(io(path)(e)),
    }
}

fn sync(path: &Path) -> Result<()> {
    File::open(path).and_then(|dir| dir.sync_all()).map_err(io(path))
}

fn check_info(directory: &[u8], info: &[u8]) -> Result<()> {
    let header = Directory::new(directory).map_err(invalid)?.header;
    if <[u8; 32]>::from(Sha256::digest(info)) != header.info {
        return Err(invalid("package.json is not that of the directory"));
    }
    Ok(())
}

fn open<F: Fn(&str) -> io::Result<File>>(fetch: &F, item: &Item) -> Result<File> {
    fetch(&hex(&item.object)).map_err(|e| item.wrong(e))
}

/// Installs the download cells `cells`, or all, with the groups of `profiles`, or all, of the
/// package of `directory` and `info` into `target`. `fetch` opens a published object by its
/// SHA-256 in hex; `threads` workers decompress and check the objects.
pub fn install<F>(
    target: &Path,
    (directory, info): (&[u8], &[u8]),
    (cells, profiles): (Option<&[u32]>, Option<&[String]>),
    threads: usize,
    fetch: F,
) -> Result<Report>
where
    F: Fn(&str) -> io::Result<File> + Sync,
{
    fs::create_dir_all(target).map_err(io(target))?;
    let lock_path = target.join(LOCK);
    let lock =
        fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).map_err(io(&lock_path))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(Error::Locked(lock_path)),
        Err(fs::TryLockError::Error(error)) => return Err(io(&lock_path)(error)),
    }
    check_info(directory, info)?;
    let plan = plan(directory, info, cells, profiles)?;
    keep(&target.join("directory.bin"), directory)?;
    keep(&target.join("package.json"), info)?;
    let dir = target.join(CELLS);
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    // The files of a stopped install: step 5 deletes them too, but only after a complete install,
    // so a failed one would keep their disk.
    for entry in fs::read_dir(&dir).map_err(io(&dir))? {
        let path = entry.map_err(io(&dir))?.path();
        if path.extension().is_some_and(|extension| extension == "part") {
            fs::remove_file(&path).map_err(io(&path))?;
        }
    }
    let missing = plan.missing(target);
    each(&missing.files, threads, |item, decoder, buffer| {
        let object = open(&fetch, item)?;
        let (part, path) = (dir.join(format!("{}.part", item.name)), dir.join(&item.name));
        // The file starts with the first bytes of the part, so an object that `stream` refuses at
        // its header leaves no file.
        let mut file = None;
        stream(item, &object, decoder, buffer, |bytes| {
            let file = match &mut file {
                Some(file) => file,
                None => file.insert(File::create(&part).map_err(io(&part))?),
            };
            file.write_all(bytes).map_err(io(&part))
        })?;
        let file = match file {
            Some(file) => file,
            None => File::create(&part).map_err(io(&part))?,
        };
        file.sync_all().map_err(io(&part))?;
        drop_cache(&file);
        drop_cache(&object);
        drop(file);
        fs::rename(&part, &path).map_err(io(&path))
    })?;
    sync(&dir)?;
    let mut marker = cells::INSTALL_MAGIC.to_vec();
    marker.extend(Sha256::digest(directory));
    marker.extend(plan.groups.to_le_bytes());
    let mut records = plan.records.clone();
    records.sort_unstable();
    marker.extend((records.len() as u32).to_le_bytes());
    records.iter().for_each(|record| marker.extend(record.to_le_bytes()));
    replace(&target.join(MARKER), &marker)?;
    sync(target)?;
    let named: std::collections::HashSet<&str> = plan.files.iter().map(|item| item.name.as_str()).collect();
    let mut removed = 0;
    for entry in fs::read_dir(&dir).map_err(io(&dir))? {
        let entry = entry.map_err(io(&dir))?;
        if !entry.file_name().to_str().is_some_and(|name| named.contains(name)) {
            fs::remove_file(entry.path()).map_err(io(&entry.path()))?;
            removed += 1;
        }
    }
    Ok(Report {
        files: missing.files.len() as u64,
        bytes: installed_bytes(&missing),
        object_bytes: object_bytes(&missing),
        removed,
    })
}

/// Checks each object of the package of `directory` and `info` as `install` does, without a file.
pub fn check<F>(directory: &[u8], info: &[u8], threads: usize, fetch: F) -> Result<Report>
where
    F: Fn(&str) -> io::Result<File> + Sync,
{
    check_info(directory, info)?;
    let plan = plan(directory, info, None, None)?;
    each(&plan.files, threads, |item, decoder, buffer| {
        let object = open(&fetch, item)?;
        stream(item, &object, decoder, buffer, |_| Ok(()))?;
        drop_cache(&object);
        Ok(())
    })?;
    Ok(Report {
        files: plan.files.len() as u64,
        bytes: installed_bytes(&plan),
        object_bytes: object_bytes(&plan),
        removed: 0,
    })
}

/// The installed cells and groups that `install.bin` of `target` names, when it names the
/// directory of `target`.
pub fn installed(target: &Path) -> Result<(Vec<u8>, Plan)> {
    let read = |name: &str| fs::read(target.join(name)).map_err(io(target));
    let (directory, marker) = (read("directory.bin")?, read(MARKER)?);
    let (groups, records) = cells::marker(&directory, &marker).map_err(invalid)?;
    let files = items(&Directory::new(&directory).map_err(invalid)?, &records, groups)?;
    Ok((directory, Plan { groups, records, files }))
}

/// Reads each installed file of `target` again. Returns the names of the files whose SHA-256 or
/// size differs from the directory, or that are missing.
pub fn scrub(target: &Path) -> Result<Vec<String>> {
    let (_, plan) = installed(target)?;
    let mut differ = Vec::new();
    for item in &plan.files {
        let path = target.join(CELLS).join(&item.name);
        let same = File::open(&path).and_then(|file| {
            let mut source =
                Hashing { inner: io::BufReader::with_capacity(BUFFER, &file), hash: Sha256::new(), bytes: 0 };
            io::copy(&mut source, &mut io::sink())?;
            drop_cache(&file);
            Ok(<[u8; 32]>::from(source.hash.finalize()) == item.sha256 && source.bytes == u64::from(item.bytes))
        });
        if !same.unwrap_or(false) {
            differ.push(item.name.clone());
        }
    }
    Ok(differ)
}
