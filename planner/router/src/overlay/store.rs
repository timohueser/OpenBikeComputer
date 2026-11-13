//! The labels and the queue of one side of a search, each with a byte count that a search limit
//! bounds.
use crate::{Error, Result};
use std::mem::size_of;

/// The slots of a table after `reset`.
const START_SLOTS: usize = 1024;
const EMPTY: u32 = u32::MAX;

/// The label of a road on one side. A road with no label reads as all-ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Label {
    pub cost: u64,
    pub parent: u32,
    pub origin: u8,
}

pub(crate) const NONE: Label = Label { cost: u64::MAX, parent: u32::MAX, origin: u8::MAX };

/// One cache line holds a whole slot, so a probe reads one place.
#[derive(Clone, Copy)]
struct Slot {
    cost: u64,
    id: u32,
    parent: u32,
    origin: u8,
}

const FREE: Slot = Slot { cost: u64::MAX, id: EMPTY, parent: u32::MAX, origin: u8::MAX };

/// Labels by road ID: open addressing with linear probing over a power of two of slots. No array
/// spans the ID space, so sparse IDs cost nothing.
#[derive(Default)]
pub(crate) struct Labels {
    slots: Vec<Slot>,
    used: usize,
}

fn home(id: u32, mask: usize) -> usize {
    // Fibonacci hashing: the high bits of the product spread consecutive IDs.
    ((u64::from(id).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize) & mask
}

impl Labels {
    pub fn bytes(&self) -> usize {
        self.slots.capacity() * size_of::<Slot>()
    }

    /// An empty table of `START_SLOTS` slots.
    pub fn reset(&mut self) -> Result<()> {
        if self.slots.len() == START_SLOTS {
            self.slots.fill(FREE);
        } else {
            self.slots = Vec::new();
            self.slots.try_reserve_exact(START_SLOTS).map_err(|_| Error::Memory)?;
            self.slots.resize(START_SLOTS, FREE);
        }
        self.used = 0;
        Ok(())
    }

    fn find(&self, id: u32) -> usize {
        let mask = self.slots.len() - 1;
        let mut at = home(id, mask);
        while self.slots[at].id != id && self.slots[at].id != EMPTY {
            at = (at + 1) & mask;
        }
        at
    }

    pub fn get(&self, id: u32) -> Label {
        let slot = self.slots[self.find(id)];
        if slot.id == EMPTY {
            return NONE;
        }
        Label { cost: slot.cost, parent: slot.parent, origin: slot.origin }
    }

    /// Sets the label of `id`. A table that would pass `room` bytes while it grows is a `Limit`.
    pub fn set(&mut self, id: u32, label: Label, room: usize) -> Result<()> {
        if id == EMPTY {
            return Err(Error::InvalidData("A road ID of all-ones".into()));
        }
        let mut at = self.find(id);
        if self.slots[at].id == EMPTY {
            if (self.used + 1) * 2 > self.slots.len() {
                self.grow(room)?;
                at = self.find(id);
            }
            self.used += 1;
        }
        self.slots[at] = Slot { cost: label.cost, id, parent: label.parent, origin: label.origin };
        Ok(())
    }

    fn grow(&mut self, room: usize) -> Result<()> {
        let count = self.slots.len().checked_mul(2).ok_or(Error::Limit)?;
        // The old and the new array exist together while the slots move.
        if (count + self.slots.len()).saturating_mul(size_of::<Slot>()) > room {
            return Err(Error::Limit);
        }
        let mut slots = Vec::new();
        slots.try_reserve_exact(count).map_err(|_| Error::Memory)?;
        slots.resize(count, FREE);
        let old = std::mem::replace(&mut self.slots, slots);
        for slot in old.into_iter().filter(|slot| slot.id != EMPTY) {
            let at = self.find(slot.id);
            self.slots[at] = slot;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub cost: u64,
    pub road: u32,
    pub origin: u8,
}

/// A monotone bucket queue by cost: bucket `k` holds the entries whose key first differs from the
/// last popped key in bit `k - 1`.
pub(crate) struct Queue {
    buckets: [Vec<Entry>; 65],
    last: u64,
    capacity: usize,
}

impl Default for Queue {
    fn default() -> Self {
        Self { buckets: std::array::from_fn(|_| Vec::new()), last: 0, capacity: 0 }
    }
}

impl Queue {
    pub fn bytes(&self) -> usize {
        self.capacity * size_of::<Entry>()
    }

    /// A queue that would pass `room` bytes is a `Limit`.
    pub fn push(&mut self, entry: Entry, room: usize) -> Result<()> {
        if entry.cost < self.last {
            return Err(Error::InvalidData("Nonmonotone search priority".into()));
        }
        let bucket = &mut self.buckets[(64 - (entry.cost ^ self.last).leading_zeros()) as usize];
        if bucket.len() == bucket.capacity() {
            let wanted = bucket.capacity().saturating_mul(2).max(4);
            // Both allocations can exist while a bucket grows or moves.
            if self.capacity.saturating_add(wanted).saturating_mul(size_of::<Entry>()) > room {
                return Err(Error::Limit);
            }
            let old = bucket.capacity();
            bucket.try_reserve_exact(wanted - bucket.len()).map_err(|_| Error::Memory)?;
            self.capacity += bucket.capacity() - old;
        }
        bucket.push(entry);
        Ok(())
    }

    /// The entry with the least cost, after the entries of the first full bucket move down.
    pub fn front(&mut self, room: usize) -> Result<Option<Entry>> {
        if self.buckets[0].is_empty() {
            let Some(index) = (1..self.buckets.len()).find(|&i| !self.buckets[i].is_empty()) else {
                return Ok(None);
            };
            self.last = self.buckets[index].iter().map(|entry| entry.cost).min().unwrap_or(self.last);
            let moved = std::mem::take(&mut self.buckets[index]);
            self.capacity -= moved.capacity();
            for &entry in &moved {
                self.push(entry, room.saturating_sub(moved.capacity() * size_of::<Entry>()))?;
            }
        }
        Ok(self.buckets[0].last().copied())
    }

    pub fn pop(&mut self) -> Option<Entry> {
        self.buckets[0].pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cmp::Reverse, collections::BinaryHeap, collections::HashMap};

    fn random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn labels_keep_each_road_through_growth_and_reset() {
        let mut labels = Labels::default();
        let mut state = 7;
        for round in 0..3 {
            labels.reset().unwrap();
            assert_eq!(labels.bytes(), START_SLOTS * 24);
            let mut oracle = HashMap::new();
            // Sparse IDs up to 2^32 - 2, some set twice, and one run of consecutive IDs.
            let ids = (0..20_000u32)
                .map(|k| if k % 4 == 0 { k + round } else { (random(&mut state) % u64::from(EMPTY)) as u32 })
                .chain([EMPTY - 1, 0]);
            for (k, id) in ids.enumerate() {
                let label = Label { cost: k as u64, parent: id ^ 1, origin: (k % 7) as u8 };
                labels.set(id, label, usize::MAX).unwrap();
                oracle.insert(id, label);
            }
            assert_eq!(labels.used, oracle.len());
            for (&id, &label) in &oracle {
                assert_eq!(labels.get(id), label);
            }
            assert_eq!(labels.get(EMPTY - 2).cost == u64::MAX, !oracle.contains_key(&(EMPTY - 2)));
        }
        labels.reset().unwrap();
        assert_eq!(labels.get(0), NONE);
        assert!(matches!(labels.set(EMPTY, NONE, usize::MAX), Err(Error::InvalidData(_))));
        let room = START_SLOTS * 24 * 2;
        let ids = 0..START_SLOTS as u32;
        let result =
            ids.map(|id| labels.set(id, Label { cost: 1, parent: 0, origin: 0 }, room)).collect::<Result<()>>();
        assert!(matches!(result, Err(Error::Limit)));
    }

    #[test]
    fn the_queue_matches_a_sorted_oracle_and_refuses_a_key_below_the_last() {
        let mut queue;
        let mut state = 41u64;
        for offset in [0, 100_000, u64::MAX - 1_000_000] {
            queue = Queue::default();
            let mut oracle = BinaryHeap::new();
            let mut last = offset;
            for step in 0..20_000 {
                if step % 3 != 0 || oracle.is_empty() {
                    let cost = last + random(&mut state) % 16;
                    queue.push(Entry { cost, road: step, origin: 0 }, usize::MAX).unwrap();
                    oracle.push(Reverse(cost));
                } else {
                    let actual = queue.front(usize::MAX).unwrap().unwrap().cost;
                    assert_eq!(actual, oracle.pop().unwrap().0);
                    assert_eq!(queue.pop().unwrap().cost, actual);
                    last = actual;
                }
            }
            while let Some(Reverse(expected)) = oracle.pop() {
                assert_eq!(queue.front(usize::MAX).unwrap().unwrap().cost, expected);
                queue.pop();
            }
            assert!(queue.front(usize::MAX).unwrap().is_none());
            if offset > 0 {
                let below = Entry { cost: offset - 1, road: 0, origin: 0 };
                assert!(matches!(queue.push(below, usize::MAX), Err(Error::InvalidData(_))));
            }
        }
        assert_eq!(size_of::<Entry>(), 16);
        let mut queue = Queue::default();
        assert!(matches!(queue.push(Entry { cost: 0, road: 0, origin: 0 }, 1), Err(Error::Limit)));
        queue.push(Entry { cost: 0, road: 0, origin: 0 }, 64).unwrap();
        assert_eq!(queue.bytes(), 64);
    }
}
