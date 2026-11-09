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

plain!(u8, u32, u64, i64, bool, [i32; 2], Option<u8>, Point, Surface);

impl<A: Plain, B: Plain> sealed::Sealed for (A, B) {}
impl<A: Plain, B: Plain> Plain for (A, B) {}

/// The smallest file of a column.
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

    /// Value `i` of the result is value `order[i]` of `self`.
    pub fn permuted(&self, order: &[u32]) -> Self {
        let values = self.values();
        order.iter().map(|&i| values[i as usize]).collect()
    }

    fn capacity(&self) -> usize {
        self.file.as_ref().map_or(0, |mapped| mapped.map.len() / size_of::<T>())
    }

    /// Doubles the file until it holds `additional` more values, so that growth is amortized O(1).
    fn reserve(&mut self, additional: usize) {
        let needed = self.len + additional;
        if needed <= self.capacity() {
            return;
        }
        let bytes = (needed.max(2 * self.capacity()) * size_of::<T>()).max(FIRST_BYTES);
        let file = match self.file.take() {
            Some(Mapped { map, file }) => {
                drop(map);
                file
            }
            None => work::File::create().unwrap_or_else(|e| panic!("create a work file: {e}")),
        };
        let map = map(&file, bytes).unwrap_or_else(|e| panic!("grow the work file {}: {e}", file.path().display()));
        self.file = Some(Mapped { map, file });
    }
}

fn map(file: &work::File, bytes: usize) -> std::io::Result<MmapMut> {
    let handle = std::fs::OpenOptions::new().read(true).write(true).open(file.path())?;
    handle.set_len(bytes as u64)?;
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

    /// Lists of the given lengths, each value `value`.
    pub fn filled(lengths: impl IntoIterator<Item = usize>, value: T) -> Self {
        let mut end = 0;
        let ends = lengths
            .into_iter()
            .map(|length| {
                end += length as u64;
                end
            })
            .collect();
        Self { ends, values: std::iter::repeat_n(value, end as usize).collect() }
    }

    pub fn get(&self, index: usize) -> &[T] {
        let range = self.range(index);
        &self.values.values()[range]
    }

    pub fn get_mut(&mut self, index: usize) -> &mut [T] {
        let range = self.range(index);
        &mut self.values.values_mut()[range]
    }

    fn range(&self, index: usize) -> std::ops::Range<usize> {
        let ends = self.ends.values();
        let start = if index == 0 { 0 } else { ends[index - 1] as usize };
        start..ends[index] as usize
    }

    /// List `i` of the result is list `order[i]` of `self`.
    pub fn permuted(&self, order: &[u32]) -> Self {
        let mut lists = Self::default();
        for &i in order {
            lists.push(self.get(i as usize));
        }
        lists
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
