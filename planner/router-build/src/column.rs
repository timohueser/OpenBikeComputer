//! Flat containers of fixed-width values in memory-mapped work files, the storage of the large
//! tables of the bake, and the stored columns of its work layers.

use crate::work;
use memmap2::{Mmap, MmapMut};
use planner_router::model::Point;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

#[cfg(target_endian = "big")]
compile_error!("a stored column holds little-endian values: the bake runs only on a little-endian target");

/// A fixed-width value without references or heap data, which a work file can hold.
pub trait Plain: Copy + sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
}

macro_rules! plain {
    ($($type:ty),*) => {
        $(impl sealed::Sealed for $type {} impl Plain for $type {})*
    };
}

plain!(bool, [i32; 2], [u8; 2]);

impl<A: Plain, B: Plain> sealed::Sealed for (A, B) {}
impl<A: Plain, B: Plain> Plain for (A, B) {}

/// A value that a stored column holds: an integer, or a `#[repr(C)]` record of integers whose
/// size is the sum of its fields, so the file has no byte that the bake did not write.
/// `Point` also holds its `f32` height.
pub trait Stored: Plain {
    /// The type in `layout.json`.
    const NAME: &'static str;
}

macro_rules! stored {
    ($($type:ty),*) => {
        $(
            impl sealed::Sealed for $type {}
            impl Plain for $type {}
            impl Stored for $type {
                const NAME: &'static str = stringify!($type);
            }
        )*
    };
}

stored!(u8, u16, u32, u64, i32, i64, crate::source::NodeRef, crate::source::WayFacts, crate::source::Rule, crate::Turn);

impl sealed::Sealed for Point {}
impl Plain for Point {}
impl Stored for Point {
    const NAME: &'static str = "Point";
}

const _: () = assert!(size_of::<Point>() == 4 + 4 + 4);

/// The smallest file of a column, and its smallest growth step.
const FIRST_BYTES: usize = 1 << 16;

/// Invariant: the first `len` slots of the mapping hold values that `extend` wrote, and nothing
/// else maps or writes the file.
pub struct Column<T> {
    len: usize,
    /// None until the column takes its first value.
    file: Option<Mapped>,
    values: PhantomData<T>,
}

enum Mapped {
    Work {
        /// Dropped before `file` removes the file: Windows cannot remove or resize a mapped file.
        map: MmapMut,
        file: work::File,
    },
    /// A stored file, read-only: `push` and `values_mut` panic.
    Stored(Mmap),
}

impl<T> Default for Column<T> {
    fn default() -> Self {
        Self { len: 0, file: None, values: PhantomData }
    }
}

impl<T: Plain> Column<T> {
    pub fn filled(len: usize, value: T) -> Self {
        std::iter::repeat_n(value, len).collect()
    }

    pub fn push(&mut self, value: T) {
        self.extend(&[value]);
    }

    pub fn extend(&mut self, values: &[T]) {
        if values.is_empty() {
            return;
        }
        self.reserve(values.len());
        let Some(Mapped::Work { map, .. }) = &mut self.file else { unreachable!("reserve maps a work file") };
        // SAFETY: `reserve` made room for `values` after the first `len` slots, and a mapping is
        // page-aligned, so every slot is aligned for `T`.
        unsafe {
            let end = map.as_mut_ptr().cast::<T>().add(self.len);
            std::ptr::copy_nonoverlapping(values.as_ptr(), end, values.len());
        }
        self.len += values.len();
    }

    pub fn pop(&mut self) -> Option<T> {
        let value = *self.values().last()?;
        self.len -= 1;
        Some(value)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn values(&self) -> &[T] {
        let start = match &self.file {
            None => return &[],
            Some(Mapped::Work { map, .. }) => map.as_ptr(),
            Some(Mapped::Stored(map)) => map.as_ptr(),
        };
        // SAFETY: see the invariant of `Column`; `open` checks the size of a stored file.
        unsafe { std::slice::from_raw_parts(start.cast(), self.len) }
    }

    pub fn values_mut(&mut self) -> &mut [T] {
        match &mut self.file {
            None => &mut [],
            // SAFETY: see the invariant of `Column`.
            Some(Mapped::Work { map, .. }) => unsafe {
                std::slice::from_raw_parts_mut(map.as_mut_ptr().cast(), self.len)
            },
            Some(Mapped::Stored(_)) => panic!("a stored column is read-only"),
        }
    }

    /// Appends value `order[i]` of `source` for each `i`.
    pub fn extend_permuted(&mut self, source: &Self, order: &[u32]) {
        self.reserve(order.len());
        let values = source.values();
        for &i in order {
            self.push(values[i as usize]);
        }
    }

    fn capacity(&self) -> usize {
        match &self.file {
            None => 0,
            Some(Mapped::Work { map, .. }) => map.len() / size_of::<T>(),
            Some(Mapped::Stored(_)) => self.len,
        }
    }

    /// Grows the file until it holds `additional` more values. A remap copies no data, so a step
    /// of 1/8 of the file keeps growth amortized O(1), and a column takes at most about 1/8 more
    /// disk than its values.
    fn reserve(&mut self, additional: usize) {
        let needed = self.len + additional;
        if needed <= self.capacity() {
            return;
        }
        let (file, old) = match self.file.take() {
            Some(Mapped::Work { map, file }) => (file, map.len()),
            Some(Mapped::Stored(_)) => panic!("a stored column is read-only"),
            None => (work::File::create().unwrap_or_else(|e| panic!("create a work file: {e}")), 0),
        };
        let bytes = (needed * size_of::<T>()).max(old + (old / 8).max(FIRST_BYTES));
        let map =
            map(&file, old, bytes).unwrap_or_else(|e| panic!("grow the work file {}: {e}", file.path().display()));
        self.file = Some(Mapped::Work { map, file });
    }
}

impl<T: Stored> Column<T> {
    /// Moves the values into the file `path`: a rename of the work file, cut to its values, or a
    /// copy where the rename fails, such as across file systems.
    pub fn save(mut self, path: &Path) -> std::io::Result<()> {
        let bytes = self.len * size_of::<T>();
        match self.file.take() {
            Some(Mapped::Work { map, file }) => {
                drop(map);
                fs::OpenOptions::new().write(true).open(file.path())?.set_len(bytes as u64)?;
                if fs::rename(file.path(), path).is_err() {
                    fs::copy(file.path(), path)?;
                }
                Ok(())
            }
            Some(Mapped::Stored(map)) => fs::write(path, &map[..bytes]),
            None => fs::write(path, []),
        }
    }

    /// Maps the file `path` read-only.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = fs::File::open(path)?;
        let bytes = file.metadata()?.len() as usize;
        if !bytes.is_multiple_of(size_of::<T>()) {
            let message = format!("{} is not a whole number of {}", path.display(), T::NAME);
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message));
        }
        if bytes == 0 {
            return Ok(Self::default());
        }
        // SAFETY: a store object is never changed in place: a new version is a new file.
        let map = unsafe { Mmap::map(&file)? };
        Ok(Self { len: bytes / size_of::<T>(), file: Some(Mapped::Stored(map)), values: PhantomData })
    }
}

/// Writes zeros from byte `old` to byte `bytes`, then maps the file. A sparse file would fault on a
/// store into the mapping when the disk is full; a write gives an error instead.
fn map(file: &work::File, old: usize, bytes: usize) -> std::io::Result<MmapMut> {
    use std::io::{Read, Seek, SeekFrom};
    let mut handle = std::fs::OpenOptions::new().read(true).write(true).open(file.path())?;
    handle.seek(SeekFrom::Start(old as u64))?;
    std::io::copy(&mut std::io::repeat(0).take((bytes - old) as u64), &mut handle)?;
    // SAFETY: the file is the column's own, in the bake's own folder: no one else changes it.
    unsafe { MmapMut::map_mut(&handle) }
}

impl<T: Plain> FromIterator<T> for Column<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let values = values.into_iter();
        let mut column = Self::default();
        column.reserve(values.size_hint().0);
        for value in values {
            column.push(value);
        }
        column
    }
}

impl<T: Plain> Clone for Column<T> {
    fn clone(&self) -> Self {
        let mut column = Self::default();
        column.extend(self.values());
        column
    }
}

impl<T: Plain + std::fmt::Debug> std::fmt::Debug for Column<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.values()).finish()
    }
}

/// `values` cut into consecutive parts of `lengths`.
pub fn split_mut<T>(mut values: &mut [T], lengths: impl IntoIterator<Item = usize>) -> Vec<&mut [T]> {
    let parts = lengths.into_iter().map(|length| {
        let (part, rest) = std::mem::take(&mut values).split_at_mut(length);
        values = rest;
        part
    });
    parts.collect()
}

/// Many variable-length lists in one column.
#[derive(Clone, Debug)]
pub struct Lists<T: Plain> {
    ends: Column<u64>,
    values: Column<T>,
}

impl<T: Plain> Default for Lists<T> {
    fn default() -> Self {
        Self { ends: Column::default(), values: Column::default() }
    }
}

impl<T: Plain> Lists<T> {
    pub fn push(&mut self, list: &[T]) {
        self.values.extend(list);
        self.ends.push(self.values.values().len() as u64);
    }

    /// Appends a list of `length` copies of `value`, a slot that `get_mut` fills later.
    pub fn push_filled(&mut self, length: usize, value: T) {
        self.values.reserve(length);
        for _ in 0..length {
            self.values.push(value);
        }
        self.ends.push(self.values.len() as u64);
    }

    pub fn get(&self, index: usize) -> &[T] {
        let range = self.range(index);
        &self.values.values()[range]
    }

    pub fn get_mut(&mut self, index: usize) -> &mut [T] {
        let range = self.range(index);
        &mut self.values.values_mut()[range]
    }

    /// The number of lists.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// The positions of list `index` in the values of all lists.
    pub fn range(&self, index: usize) -> std::ops::Range<usize> {
        let ends = self.ends.values();
        let start = if index == 0 { 0 } else { ends[index - 1] as usize };
        start..ends[index] as usize
    }

    /// The values of all lists, list after list.
    pub fn values(&self) -> &[T] {
        self.values.values()
    }

    /// Appends list `order[i]` of `source` for each `i`.
    pub fn extend_permuted(&mut self, source: &Self, order: &[u32]) {
        for &i in order {
            self.push(source.get(i as usize));
        }
    }
}

/// The stored columns of one folder of a work layer, with the type and the count of each file:
/// `layout.json`, `{"format": 1, "files": {name: {"type", "len"}}}`. A step saves each column into
/// its folder and writes the layout last; a reader opens a file only when its type and count are
/// those of the layout. `Lists` take two files, `<name>.ends` and `<name>.values`.
#[derive(Default)]
pub struct Layout {
    files: BTreeMap<String, (String, u64)>,
    /// The folder that a step saves into.
    dir: Option<PathBuf>,
    /// The file of each name of a read folder: in the folder, or an object of the store.
    paths: BTreeMap<String, PathBuf>,
}

impl Layout {
    pub const FILE: &'static str = "layout.json";
    const FORMAT: u64 = 1;

    /// A layout to save into the empty folder `dir`.
    pub fn create(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self { files: BTreeMap::new(), dir: Some(dir.to_path_buf()), paths: BTreeMap::new() })
    }

    /// The layout of a folder whose files are at `paths`, by their names in the folder.
    pub fn read(paths: BTreeMap<String, PathBuf>) -> Result<Self, String> {
        let path = paths.get(Self::FILE).ok_or("the layer has no layout.json")?;
        let text = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value: Value = serde_json::from_slice(&text).map_err(|e| format!("layout.json: {e}"))?;
        if value["format"].as_u64() != Some(Self::FORMAT) {
            return Err(format!("layout.json has format {}; this bake reads {}", value["format"], Self::FORMAT));
        }
        let entries = value["files"].as_object().ok_or("layout.json has no files")?;
        let mut files = BTreeMap::new();
        for (name, entry) in entries {
            let (kind, len) = (entry["type"].as_str(), entry["len"].as_u64());
            let (Some(kind), Some(len)) = (kind, len) else { return Err(format!("layout.json: bad entry {name}")) };
            files.insert(name.clone(), (kind.to_string(), len));
        }
        Ok(Self { files, dir: None, paths })
    }

    /// The path of the file `name` of a read folder, or of a folder to save into.
    pub fn path(&self, name: &str) -> Result<PathBuf, String> {
        match (&self.dir, self.paths.get(name)) {
            (Some(dir), _) => Ok(dir.join(name)),
            (None, Some(path)) => Ok(path.clone()),
            (None, None) => Err(format!("the layer has no file {name}")),
        }
    }

    pub fn save<T: Stored>(&mut self, name: &str, column: Column<T>) -> Result<(), String> {
        let (path, len) = (self.path(name)?, column.len() as u64);
        column.save(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.files.insert(name.into(), (T::NAME.into(), len));
        Ok(())
    }

    pub fn save_lists<T: Stored>(&mut self, name: &str, lists: Lists<T>) -> Result<(), String> {
        self.save(&format!("{name}.ends"), lists.ends)?;
        self.save(&format!("{name}.values"), lists.values)
    }

    /// Writes `layout.json`, after the files that it lists.
    pub fn finish(self) -> Result<(), String> {
        let files: serde_json::Map<String, Value> =
            self.files.into_iter().map(|(name, (kind, len))| (name, json!({"type": kind, "len": len}))).collect();
        let bytes = serde_json::to_vec(&json!({"format": Self::FORMAT, "files": files})).expect("JSON values");
        let path = self.dir.as_ref().expect("a layout to save").join(Self::FILE);
        fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn open<T: Stored>(&self, name: &str) -> Result<Column<T>, String> {
        let (kind, len) = self.files.get(name).ok_or(format!("layout.json lists no file {name}"))?;
        if kind != T::NAME {
            return Err(format!("{name} holds {kind}, not {}", T::NAME));
        }
        let path = self.path(name)?;
        let column = Column::open(&path).map_err(|e| format!("{name}: {e}"))?;
        if column.len() as u64 != *len {
            return Err(format!("{name} holds {} values, layout.json says {len}", column.len()));
        }
        Ok(column)
    }

    pub fn open_lists<T: Stored>(&self, name: &str) -> Result<Lists<T>, String> {
        let (ends, values) = (self.open(&format!("{name}.ends"))?, self.open(&format!("{name}.values"))?);
        if ends.values().last().copied().unwrap_or(0) != values.len() as u64 {
            return Err(format!("{name}.ends does not end at the values"));
        }
        Ok(Lists { ends, values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_reads_back_what_it_took_across_growth() {
        type Value = (u64, u32);
        let first = FIRST_BYTES / size_of::<Value>();
        let taken: Vec<Value> = (0..3 * first as u64).map(|i| (i * 7919, i as u32)).collect();
        let mut column = Column::default();
        for &value in &taken {
            column.push(value);
        }
        assert!(column.capacity() > first, "the file grew");
        assert_eq!(column.values(), taken);
        let Some(Mapped::Work { file, .. }) = &column.file else { panic!("a work column") };
        let path = file.path().to_owned();
        assert!(path.exists());
        drop(column);
        assert!(!path.exists(), "a dropped column removes its file");
    }

    #[test]
    fn lists_read_back_what_they_took() {
        let taken: [&[i64]; 4] = [&[3, 1], &[], &[i64::MIN], &[7, 7, 7]];
        let mut lists = Lists::default();
        for list in taken {
            lists.push(list);
        }
        assert_eq!((0..taken.len()).map(|i| lists.get(i)).collect::<Vec<_>>(), taken);
    }

    #[test]
    fn a_saved_column_opens_with_its_values() {
        let dir = std::env::temp_dir().join(format!("obc-column-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let numbers: Vec<u32> = (0..100_000).map(|i| i * 7919).collect();
        let shapes: [&[Point]; 3] = [&[Point { lat: 1, lon: 2, elevation: 3.5 }], &[], &[Point::default(); 2]];
        let facts = crate::source::WayFacts { class: 3, ranks: [2, 0], ..Default::default() };
        let mut layout = Layout::create(&dir).unwrap();
        layout.save("numbers", numbers.iter().copied().collect()).unwrap();
        let mut lists = Lists::default();
        shapes.iter().for_each(|shape| lists.push(shape));
        layout.save_lists("shapes", lists).unwrap();
        layout.save("facts", Column::filled(2, facts)).unwrap();
        layout.finish().unwrap();
        let paths = fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path());
        let paths = paths.map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), path)).collect();

        let layout = Layout::read(paths).unwrap();
        assert_eq!(layout.open::<u32>("numbers").unwrap().values(), numbers);
        let lists = layout.open_lists::<Point>("shapes").unwrap();
        assert_eq!((0..3).map(|i| lists.get(i)).collect::<Vec<_>>(), shapes);
        assert_eq!(layout.open::<crate::source::WayFacts>("facts").unwrap().values(), [facts; 2]);
        assert!(layout.open::<u64>("numbers").unwrap_err().contains("holds u32"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
