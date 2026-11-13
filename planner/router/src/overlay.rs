//! The road search on an install of format 10: a search from both ends over the turns of the
//! level-1 cells, with all cells open. It gives the least (cost, end choice, start choice) path.
mod store;

use crate::{
    cells::values,
    host::Host,
    region::{Installed, Junction, Shared},
    Error, Result,
};
use std::{cell::Cell, collections::HashMap};
use store::{Entry, Label, Labels, Queue};

/// The search limit of a worker: the most bytes of the labels, queues and reverse turns of one
/// search.
pub const SEARCH_BYTES: usize = 768 * 1024 * 1024;

/// A search enters or leaves at the arrival state of a road, which is its road id.
pub struct Seed {
    pub node: u32,
    pub cost: u64,
    pub choice: u8,
}

#[derive(Clone, Copy)]
pub struct Query<'a> {
    pub starts: &'a [Seed],
    pub ends: &'a [Seed],
    pub ceiling: u64,
    pub max_roads: usize,
    /// The room of both search queues now.
    pub heap_bytes: usize,
    /// The room of both search queues on an idle service, at least `heap_bytes`.
    pub heap_limit: usize,
    pub cancelled: &'a dyn Fn() -> bool,
}

pub struct Found {
    pub cost: u64,
    pub roads: Vec<u32>,
    pub source: usize,
    pub target: usize,
}

/// The work of searches: the labels that they settled and the most bytes that one held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub settled: usize,
    pub peak_bytes: usize,
}

/// The turns into the roads of one level-1 cell: target `t` has the pairs (source road, turn) at
/// `first[t]..first[t + 1]`.
struct Reverse {
    first: Vec<u32>,
    pairs: Vec<(u32, u32)>,
}

impl Reverse {
    fn bytes(&self) -> usize {
        self.first.capacity() * 4 + self.pairs.capacity() * 8
    }
}

/// The memory of the searches of one worker.
#[derive(Default)]
pub struct Workspace {
    labels: [Labels; 2],
    queues: [Queue; 2],
    /// The reverse turns of the level-1 cells that the backward side entered, by download cell
    /// record and level-1 index.
    entered: HashMap<(u32, u32), Reverse>,
    entered_bytes: usize,
    arcs: Vec<(u32, u64)>,
    stats: Cell<Stats>,
}

impl Workspace {
    /// The counts of the searches since the last call.
    pub fn take_stats(&self) -> Stats {
        self.stats.take()
    }

    fn bytes(&self) -> usize {
        let [a, b] = &self.labels;
        let [c, d] = &self.queues;
        a.bytes() + b.bytes() + c.bytes() + d.bytes() + self.entered_bytes
    }
}

fn reserved<T: Clone>(count: usize, value: T) -> Result<Vec<T>> {
    let mut vec = Vec::new();
    vec.try_reserve_exact(count).map_err(|_| Error::Memory)?;
    vec.resize(count, value);
    Ok(vec)
}

fn invalid(what: &str) -> Error {
    Error::InvalidData(format!("Invalid route cell data: {what}"))
}

struct Search<'a, H> {
    shared: &'a Shared<H>,
    installed: &'a Installed,
    work: &'a mut Workspace,
    limit: usize,
    best: u64,
    /// The choices of the end and the start seed of `best`.
    roots: (u8, u8),
    meeting: Option<u32>,
    pops: usize,
    peak: usize,
}

/// The least `(cost, end choice, start choice)` path of `query` on `shared` with the profile
/// `installed`, in at most `limit` bytes.
pub(crate) fn search<H: Host>(
    shared: &Shared<H>,
    work: &mut Workspace,
    installed: &Installed,
    query: Query<'_>,
    limit: usize,
) -> Result<Option<Found>> {
    if query.starts.is_empty() || query.ends.is_empty() {
        return Ok(None);
    }
    work.queues = Default::default();
    work.entered = HashMap::new();
    work.entered_bytes = 0;
    for labels in &mut work.labels {
        labels.reset()?;
    }
    let mut search = Search {
        shared,
        installed,
        work,
        limit,
        best: query.ceiling,
        roots: (u8::MAX, u8::MAX),
        meeting: None,
        pops: 0,
        peak: 0,
    };
    let result = search.run(query);
    let Search { work, pops, peak, .. } = search;
    let stats = work.stats.get();
    work.stats.set(Stats { settled: stats.settled + pops, peak_bytes: stats.peak_bytes.max(peak) });
    result
}

impl<H: Host> Search<'_, H> {
    fn run(&mut self, query: Query<'_>) -> Result<Option<Found>> {
        let Query { starts, ends, max_roads, cancelled, .. } = query;
        self.peak = self.work.bytes();
        for (side, seeds) in [starts, ends].into_iter().enumerate() {
            for seed in seeds {
                if seed.cost == u64::MAX || !self.shared.has(seed.node) {
                    return Err(Error::InvalidData("Invalid search seed".into()));
                }
                let label = self.work.labels[side].get(seed.node);
                if (seed.cost, seed.choice) < (label.cost, label.origin) {
                    self.set(side, seed.node, Label { cost: seed.cost, parent: u32::MAX, origin: seed.choice })?;
                }
            }
        }
        loop {
            if self.pops.is_multiple_of(1024) && cancelled() {
                return Err(Error::Cancelled);
            }
            let mut tops = [u64::MAX; 2];
            for (side, top) in tops.iter_mut().enumerate() {
                while let Some(entry) = self.front(side)? {
                    let label = self.work.labels[side].get(entry.road);
                    if (label.cost, label.origin) == (entry.cost, entry.origin) {
                        *top = entry.cost;
                        break;
                    }
                    self.work.queues[side].pop();
                }
            }
            if tops.contains(&u64::MAX) || tops[0].saturating_add(tops[1]) > self.best {
                break;
            }
            let side = usize::from(tops[1] < tops[0]);
            let entry = self.work.queues[side].pop().ok_or_else(|| invalid("an empty queue"))?;
            self.pops += 1;
            self.meet(entry.road)?;
            self.expand(side, entry)?;
        }
        let Some(meeting) = self.meeting else { return Ok(None) };
        let labels = &self.work.labels;
        let mut roads = vec![meeting];
        let parent = |side: usize, road: u32, roads: &Vec<u32>| {
            let label = labels[side].get(road);
            if label.parent == u32::MAX {
                return Ok(None);
            }
            if roads.len() > max_roads {
                return Err(Error::Limit);
            }
            Ok(Some(label.parent))
        };
        while let Some(road) = parent(0, *roads.last().unwrap_or(&meeting), &roads)? {
            roads.push(road);
        }
        roads.reverse();
        let witness = |side: usize, seeds: &[Seed], road: u32| {
            let label = labels[side].get(road);
            seeds
                .iter()
                .position(|seed| seed.node == road && (seed.cost, seed.choice) == (label.cost, label.origin))
                .ok_or_else(|| Error::InvalidData(format!("Missing {} witness", ["source", "target"][side])))
        };
        let source = witness(0, starts, roads[0])?;
        while let Some(road) = parent(1, *roads.last().unwrap_or(&meeting), &roads)? {
            roads.push(road);
        }
        let target = witness(1, ends, *roads.last().unwrap_or(&meeting))?;
        let mut check = starts[source].cost.checked_add(ends[target].cost).ok_or(Error::Limit)?;
        let mut arcs = std::mem::take(&mut self.work.arcs);
        for pair in roads.windows(2) {
            arcs.clear();
            self.arcs(0, pair[0], &mut arcs)?;
            let weight = arcs.iter().filter(|arc| arc.0 == pair[1]).map(|arc| arc.1).min();
            let weight = weight.ok_or_else(|| Error::InvalidData("A path road without its turn".into()))?;
            check = check.checked_add(weight).ok_or(Error::Limit)?;
        }
        self.work.arcs = arcs;
        if check != self.best {
            return Err(Error::InvalidData("Base path witness cost differs".into()));
        }
        Ok(Some(Found { cost: self.best, roads, source, target }))
    }

    /// The bytes that the search may hold besides those of a store that holds `own`.
    fn room(&self, own: usize) -> usize {
        self.limit.saturating_sub(self.work.bytes() - own)
    }

    fn front(&mut self, side: usize) -> Result<Option<Entry>> {
        let room = self.room(self.work.queues[side].bytes());
        let entry = self.work.queues[side].front(room)?;
        self.peak = self.peak.max(self.work.bytes());
        Ok(entry)
    }

    fn set(&mut self, side: usize, road: u32, label: Label) -> Result<()> {
        let room = self.room(self.work.labels[side].bytes());
        self.work.labels[side].set(road, label, room)?;
        let room = self.room(self.work.queues[side].bytes());
        self.work.queues[side].push(Entry { cost: label.cost, road, origin: label.origin }, room)?;
        self.peak = self.peak.max(self.work.bytes());
        Ok(())
    }

    /// A road with a label on both sides joins a path.
    fn meet(&mut self, road: u32) -> Result<()> {
        let [forward, backward] = [0, 1].map(|side| self.work.labels[side].get(road));
        if forward.cost == u64::MAX || backward.cost == u64::MAX {
            return Ok(());
        }
        let joined = forward.cost.checked_add(backward.cost).ok_or(Error::Limit)?;
        if (joined, (backward.origin, forward.origin)) < (self.best, self.roots) {
            (self.best, self.roots, self.meeting) = (joined, (backward.origin, forward.origin), Some(road));
        }
        Ok(())
    }

    /// Relaxes each arc of the popped road on its side.
    fn expand(&mut self, side: usize, entry: Entry) -> Result<()> {
        let mut arcs = std::mem::take(&mut self.work.arcs);
        arcs.clear();
        let result = self.arcs(side, entry.road, &mut arcs).and_then(|()| {
            for &(road, weight) in &arcs {
                let next = entry.cost.checked_add(weight).filter(|&n| n != u64::MAX).ok_or(Error::Limit)?;
                let label = self.work.labels[side].get(road);
                if next > self.best || (next, entry.origin) >= (label.cost, label.origin) {
                    continue;
                }
                self.set(side, road, Label { cost: next, parent: entry.road, origin: entry.origin })?;
                self.meet(road)?;
            }
            Ok(())
        });
        self.work.arcs = arcs;
        result
    }

    /// Forward, the arcs `road → v`; backward, the arcs `u → road`. Each with its weight: the
    /// cost of the arc's target road plus the turn cost.
    fn arcs(&mut self, side: usize, road: u32, arcs: &mut Vec<(u32, u64)>) -> Result<()> {
        let (shared, group, profile) = (self.shared, self.installed.group, self.installed.place);
        let place = shared.place(road)?;
        let (costs, cell) = shared.group(&place.cell, place.index, &place.level1, group)?;
        if side == 0 {
            let (topology, level1) = (place.topology, place.level1);
            for turn in topology.turns(&level1, place.road)? {
                let Some(penalty) = costs.turn(&cell, turn)? else { continue };
                let head = topology.head(&level1, turn)?;
                let (to, cost) = if head < level1.roads {
                    (place.cell.first_road + level1.first_road + head, costs.cost(&cell, profile, head)?)
                } else {
                    let Some(to) = shared.road_id(topology.leaving(&level1, head - level1.roads)?) else { continue };
                    let target = shared.place(to)?;
                    let (costs, cell) = shared.group(&target.cell, target.index, &target.level1, group)?;
                    (to, costs.cost(&cell, profile, target.road)?)
                };
                if let Some(cost) = cost {
                    arcs.push((to, u64::from(cost) + u64::from(penalty)));
                }
            }
            return Ok(());
        }
        let Some(cost) = costs.cost(&cell, profile, place.road)? else { return Ok(()) };
        let Some(junction) = shared.junction(&place, road)? else { return Ok(()) };
        let Junction { cell: download, topology, index, level1, target } = junction;
        let key = (download.record, index);
        if !self.work.entered.contains_key(&key) {
            let reverse = self.reverse(&topology, &level1)?;
            self.work.entered_bytes += reverse.bytes();
            self.work.entered.try_reserve(1).map_err(|_| Error::Memory)?;
            self.work.entered.insert(key, reverse);
            self.peak = self.peak.max(self.work.bytes());
        }
        let reverse = &self.work.entered[&key];
        let (costs, cell) = shared.group(&download, index, &level1, group)?;
        let (low, high) = (reverse.first[target as usize] as usize, reverse.first[target as usize + 1] as usize);
        for &(source, turn) in &reverse.pairs[low..high] {
            if let Some(penalty) = costs.turn(&cell, turn)? {
                arcs.push((download.first_road + level1.first_road + source, u64::from(cost) + u64::from(penalty)));
            }
        }
        Ok(())
    }

    /// The reverse turns of a level-1 cell: a counting sort of its `head` column.
    fn reverse(&self, topology: &crate::cells::Topology<'_>, level1: &crate::cells::Level1) -> Result<Reverse> {
        let targets = level1.roads as usize + usize::from(level1.leaving);
        let turns = level1.turns as usize;
        // `first`, its cursor, `pairs` and the decoded `head` exist together while it builds.
        let bytes = (targets + 1).saturating_mul(8).saturating_add(turns.saturating_mul(12));
        if bytes > self.room(0) {
            return Err(Error::Limit);
        }
        let (first_column, head_column) = topology.columns(level1)?;
        let mut heads = Vec::new();
        heads.try_reserve_exact(turns).map_err(|_| Error::Memory)?;
        heads.extend(values(head_column, level1.head_width));
        if heads.len() != turns {
            return Err(invalid("a head column that is not the turn count"));
        }
        let mut first = reserved(targets + 1, 0u32)?;
        for &head in &heads {
            *first
                .get_mut(head as usize + 1)
                .filter(|_| (head as usize) < targets)
                .ok_or_else(|| invalid("a turn target outside the cell"))? += 1;
        }
        for t in 0..targets {
            first[t + 1] += first[t];
        }
        let mut cursor = first.clone();
        let mut pairs = reserved(turns, (0u32, 0u32))?;
        let mut ends = values(first_column, level1.first_width);
        let mut low =
            ends.next().filter(|&value| value == 0).ok_or_else(|| invalid("a turn column that does not start at 0"))?;
        for (source, high) in (0u32..).zip(ends) {
            if high < low || high as usize > turns {
                return Err(invalid("a bad turn range"));
            }
            for turn in low..high {
                let slot = &mut cursor[heads[turn as usize] as usize];
                pairs[*slot as usize] = (source, turn);
                *slot += 1;
            }
            low = high;
        }
        if low as usize != turns {
            return Err(invalid("a turn column that does not end at the turn count"));
        }
        Ok(Reverse { first, pairs })
    }
}
