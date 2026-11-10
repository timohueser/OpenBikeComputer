//! A merge sort of fixed-width records on disk.

use crate::column::{Column, Plain};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// The values of `values` in their full order, so equal keys come out the same in every bake.
/// Each run of `work::sort_run_bytes` is sorted in place in the column's work file, then the runs
/// are merged as they are read.
pub fn sorted<T: Plain + Ord>(values: Column<T>) -> Sorted<T> {
    let bytes = crate::work::sort_run_bytes().unwrap_or_else(|e| panic!("{e}"));
    in_runs(values, (bytes / size_of::<T>()).max(1))
}

fn in_runs<T: Plain + Ord>(mut values: Column<T>, run: usize) -> Sorted<T> {
    values.values_mut().chunks_mut(run).for_each(<[T]>::sort_unstable);
    let heads = values.values().iter().step_by(run).enumerate().map(|(i, &value)| Reverse((value, i * run))).collect();
    Sorted { values, heads, run, read: 0 }
}

pub struct Sorted<T> {
    values: Column<T>,
    /// The smallest value of each run that is not read yet, with its position.
    heads: BinaryHeap<Reverse<(T, usize)>>,
    run: usize,
    read: usize,
}

impl<T: Plain + Ord> Sorted<T> {
    /// Each distinct value once, with the number of its copies.
    pub fn counted(self) -> impl Iterator<Item = (T, usize)> {
        let mut values = self.peekable();
        std::iter::from_fn(move || {
            let value = values.next()?;
            let mut count = 1;
            while values.next_if_eq(&value).is_some() {
                count += 1;
            }
            Some((value, count))
        })
    }
}

impl<T: Plain + Ord> Iterator for Sorted<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let Reverse((value, position)) = self.heads.pop()?;
        let after = position + 1;
        if after % self.run != 0 && after < self.values.values().len() {
            self.heads.push(Reverse((self.values.values()[after], after)));
        }
        self.read += 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.values.values().len() - self.read;
        (left, Some(left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sort_over_several_runs_gives_the_order_of_a_sort_in_ram() {
        let values: Vec<(u64, u32)> = (0..1000u64).map(|i| ((i * 7919) % 97, (i * 31 % 5) as u32)).collect();
        let mut expected = values.clone();
        expected.sort();
        let column = || values.iter().copied().collect::<Column<_>>();
        assert_eq!(in_runs(column(), 64).collect::<Vec<_>>(), expected);
        let counts = expected.chunk_by(|a, b| a == b).map(|copies| (copies[0], copies.len()));
        assert_eq!(in_runs(column(), 64).counted().collect::<Vec<_>>(), counts.collect::<Vec<_>>());
    }
}
