//! Flat containers of fixed-width values in memory-mapped work files, the storage of the large
//! tables of the bake.

use crate::work;
use memmap2::MmapMut;
use planner_router::model::{Point, Surface};
use std::marker::PhantomData;

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

plain!(
    u8,
    u32,
    u64,
    i64,
    bool,
    [i32; 2],
    [u8; 2],
    Option<u8>,
    Point,
    Surface,
    crate::country::Country,
    crate::source::NodeRef,
    crate::source::WayFacts
);

impl<A: Plain, B: Plain> sealed::Sealed for (A, B) {}
impl<A: Plain, B: Plain> Plain for (A, B) {}

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

struct Mapped {
    /// Dropped before `file` removes the file: Windows cannot remove or resize a mapped file.
    map: MmapMut,
    file: work::File,
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
        let map = &mut self.file.as_mut().expect("reserve maps the file").map;
        // SAFETY: `reserve` made room for `values` after the first `len` slots, and a mapping is
        // page-aligned, so every slot is aligned for `T`.
        unsafe {
            let end = map.as_mut_ptr().cast::<T>().add(self.len);
            std::ptr::copy_nonoverlapping(values.as_ptr(), end, values.len());
        }
        self.len += values.len();
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn values(&self) -> &[T] {
        match &self.file {
            None => &[],
            // SAFETY: see the invariant of `Column`.
            Some(mapped) => unsafe { std::slice::from_raw_parts(mapped.map.as_ptr().cast(), self.len) },
        }
    }

    pub fn values_mut(&mut self) -> &mut [T] {
        match &mut self.file {
            None => &mut [],
            // SAFETY: see the invariant of `Column`.
            Some(mapped) => unsafe { std::slice::from_raw_parts_mut(mapped.map.as_mut_ptr().cast(), self.len) },
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
        self.file.as_ref().map_or(0, |mapped| mapped.map.len() / size_of::<T>())
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
            Some(Mapped { map, file }) => (file, map.len()),
            None => (work::File::create().unwrap_or_else(|e| panic!("create a work file: {e}")), 0),
        };
        let bytes = (needed * size_of::<T>()).max(old + (old / 8).max(FIRST_BYTES));
        let map =
            map(&file, old, bytes).unwrap_or_else(|e| panic!("grow the work file {}: {e}", file.path().display()));
        self.file = Some(Mapped { map, file });
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
        let path = column.file.as_ref().unwrap().file.path().to_owned();
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
}
