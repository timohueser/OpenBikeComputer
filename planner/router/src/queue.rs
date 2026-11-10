//! Bounded monotone priorities for Dijkstra and feasible-potential searches.
use crate::{Error, Result};
use std::mem::size_of;

pub(crate) type Entry = (i128, u64, u8, u32);

pub(crate) struct Queue {
    buckets: [Vec<Entry>; 129],
    last: u128,
    capacity: usize,
}
impl Default for Queue {
    fn default() -> Self {
        Self { buckets: std::array::from_fn(|_| Vec::new()), last: 0, capacity: 0 }
    }
}
fn key(value: i128) -> u128 {
    (value as u128) ^ (1 << 127)
}
impl Queue {
    pub fn bytes(&self) -> usize {
        self.capacity.saturating_mul(size_of::<Entry>())
    }
    pub fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.last = 0;
    }
    pub fn shrink_to_fit(&mut self) {
        for bucket in &mut self.buckets {
            bucket.shrink_to_fit();
        }
        self.capacity = self.buckets.iter().map(Vec::capacity).sum();
    }
    /// Fails with `Limit` when the queue would pass `limit`, the room of the search on an idle
    /// service, else with `Memory` when it would pass `room`, the room now.
    pub fn push(&mut self, entry: Entry, room: usize, limit: usize) -> Result<()> {
        let key = key(entry.0);
        if key < self.last {
            return Err(Error::InvalidData("Nonmonotone search priority".into()));
        }
        let index = (128 - (key ^ self.last).leading_zeros()) as usize;
        let bucket = &mut self.buckets[index];
        if bucket.len() == bucket.capacity() {
            let wanted = bucket.capacity().saturating_mul(2).max(4);
            // Both allocations can exist while a bucket grows or redistributes.
            let bytes = self.capacity.saturating_add(wanted).saturating_mul(size_of::<Entry>());
            if bytes > limit {
                return Err(Error::Limit);
            }
            if bytes > room {
                return Err(Error::Memory);
            }
            let old = bucket.capacity();
            bucket.try_reserve_exact(wanted - bucket.len()).map_err(|_| Error::Memory)?;
            self.capacity += bucket.capacity() - old;
        }
        bucket.push(entry);
        Ok(())
    }
    pub fn front(&mut self, room: usize, limit: usize) -> Result<Option<Entry>> {
        if self.buckets[0].is_empty() {
            let Some(index) = (1..self.buckets.len()).find(|&i| !self.buckets[i].is_empty()) else {
                return Ok(None);
            };
            self.last = key(self.buckets[index].iter().map(|entry| entry.0).min().unwrap());
            while let Some(entry) = self.buckets[index].pop() {
                self.push(entry, room, limit)?;
            }
            self.capacity -= self.buckets[index].capacity();
            self.buckets[index] = Vec::new();
        }
        Ok(self.top())
    }
    pub fn top(&self) -> Option<Entry> {
        self.buckets[0].last().copied()
    }
    pub fn pop(&mut self) -> Option<Entry> {
        self.buckets[0].pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cmp::Reverse, collections::BinaryHeap};
    const MIB: usize = 16 * 1024 * 1024;
    #[test]
    fn monotone_queue_matches_a_sorted_oracle_across_reuse_extremes_and_limits() {
        let mut queue = Queue::default();
        let mut random = 41u64;
        for offset in [i128::MIN, -100_000, 0, i128::MAX - 1_000_000] {
            queue.clear();
            let mut oracle = BinaryHeap::new();
            let mut last = offset;
            for step in 0..20_000 {
                if step % 3 != 0 || oracle.is_empty() {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let priority = last + (random % 16) as i128;
                    queue.push((priority, 0, 0, 0), MIB, MIB).unwrap();
                    oracle.push(Reverse(priority));
                } else {
                    let actual = queue.front(MIB, MIB).unwrap().unwrap().0;
                    assert_eq!(actual, oracle.pop().unwrap().0);
                    assert_eq!(queue.pop().unwrap().0, actual);
                    last = actual;
                }
            }
            while let Some(Reverse(expected)) = oracle.pop() {
                assert_eq!(queue.front(MIB, MIB).unwrap().unwrap().0, expected);
                queue.pop();
            }
            assert!(queue.front(MIB, MIB).unwrap().is_none());
            assert!(matches!(queue.push((offset, 0, 0, 0), usize::MAX, usize::MAX), Err(Error::InvalidData(_))));
        }
        queue.clear();
        queue.shrink_to_fit();
        assert_eq!(queue.bytes(), 0);
        queue.clear();
        queue.push((1, 0, 0, 0), 1024, 1024).unwrap();
        assert_eq!(queue.front(1024, 1024).unwrap().unwrap().0, 1);
    }

    #[test]
    fn a_queue_over_its_idle_room_is_a_limit_and_over_its_room_now_is_memory() {
        let mut queue = Queue::default();
        assert!(matches!(queue.push((0, 0, 0, 0), 1, 1), Err(Error::Limit)));
        assert!(matches!(queue.push((0, 0, 0, 0), 1, 1024), Err(Error::Memory)));
        // Moving an entry to the first bucket needs a second allocation beside the first.
        let front = |room, limit| {
            let mut queue = Queue::default();
            queue.push((0, 0, 0, 0), room, limit).unwrap();
            queue.front(room, limit).map(|entry| entry.map(|entry| entry.0))
        };
        assert!(matches!(front(128, 128), Err(Error::Limit)));
        assert!(matches!(front(128, 1024), Err(Error::Memory)));
        assert_eq!(front(1024, 1024).unwrap(), Some(0));
    }
}
