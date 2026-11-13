//! The search on an install of format 10: a search from both ends that gives the least (cost, end
//! choice, start choice) path.
//!
//! The seed cells of a search are the level-1 cells of the roads of its seeds. A cell is closed
//! when no seed cell lies in it and it has a cost table; each other cell is open. The search
//! crosses the highest closed cell of a junction in one step, with the table, and uses the turns at
//! a junction whose cells are all open. Both sides search this one graph, and each of its arcs
//! costs what its roads cost, so the result is that of a search on the roads.
mod store;
mod unpack;

use crate::{
    cells::{values, MAX_LEVELS},
    host::Host,
    region::{Closed, Home, Installed, Junction, Place, Shared},
    Error, Result,
};
use std::{cell::Cell, collections::HashMap};
use store::{Entry, Label, Labels, Queue};
pub(crate) use unpack::unpack;

/// The search limit of a worker: the most bytes of the labels, queues and reverse turns of one
/// search. A search with cost tables holds a small part of it, so only a defect reaches it.
pub const SEARCH_BYTES: usize = 64 * 1024 * 1024;

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
    /// The most steps of the path.
    pub max_steps: usize,
    /// The room of both search queues now.
    pub heap_bytes: usize,
    /// The room of both search queues on an idle service, at least `heap_bytes`.
    pub heap_limit: usize,
    pub cancelled: &'a dyn Fn() -> bool,
}

/// One step of a path: the road that it leads to and its level. Level 0 is a turn, or the first
/// road of the path; a higher level crosses a cell of that level by its cost table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub road: u32,
    pub level: u8,
}

pub struct Found {
    pub cost: u64,
    /// The cost of the steps: `cost` less the costs of the two seeds.
    pub arcs: u64,
    pub path: Vec<Step>,
    pub source: usize,
    pub target: usize,
}

/// The work of searches and of the unpacking of their paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub searches: usize,
    /// The labels that the searches settled.
    pub settled: usize,
    /// The settled labels by the level of their expansion: 0 by the turns of the road, else by a
    /// cost table of that level.
    pub levels: [usize; MAX_LEVELS + 1],
    /// The values of the cost tables that the searches read.
    pub values: usize,
    /// The most bytes that one search held.
    pub peak_bytes: usize,
    /// The legs that were unpacked, their roads, and the labels that the unpacking settled.
    pub legs: usize,
    pub roads: usize,
    pub unpack_settled: usize,
    /// The nanoseconds of the searches and of the unpacking; 0 on WebAssembly, which has no clock.
    pub search_ns: u64,
    pub unpack_ns: u64,
}

/// Starts a clock; the call of the result gives the nanoseconds since then.
fn clock() -> impl FnOnce() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    move || {
        #[cfg(not(target_arch = "wasm32"))]
        return started.elapsed().as_nanos() as u64;
        #[cfg(target_arch = "wasm32")]
        0
    }
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

    fn count(&self, add: impl FnOnce(&mut Stats)) {
        let mut stats = self.stats.get();
        add(&mut stats);
        self.stats.set(stats);
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

/// Adds the arcs of the turns of the road of `place` to `arcs`, each with its weight: the cost of
/// its target road plus the turn cost. `leaving` gives the ID and the cost of a leaving road of
/// the level-1 cell by its place in the list, or `None` for a road that the caller leaves out.
fn turns<H: Host>(
    shared: &Shared<H>,
    installed: &Installed,
    place: &Place<'_>,
    mut leaving: impl FnMut(u32) -> Result<Option<(u32, Option<u32>)>>,
    arcs: &mut Vec<(u32, u64)>,
) -> Result<()> {
    let (topology, level1) = (place.topology, place.level1);
    let (costs, cell) = shared.group(&place.cell, place.index, &level1, installed.group)?;
    for turn in topology.turns(&level1, place.road)? {
        let Some(penalty) = costs.turn(&cell, turn)? else { continue };
        let head = topology.head(&level1, turn)?;
        let (to, cost) = if head < level1.roads {
            (place.cell.first_road + level1.first_road + head, costs.cost(&cell, installed.place, head)?)
        } else {
            let Some(found) = leaving(head - level1.roads)? else { continue };
            found
        };
        if let Some(cost) = cost {
            arcs.push((to, u64::from(cost) + u64::from(penalty)));
        }
    }
    Ok(())
}

/// The ID and the cost of road `to` for the profile `installed`.
fn target<H: Host>(shared: &Shared<H>, installed: &Installed, to: u32) -> Result<(u32, Option<u32>)> {
    let place = shared.place(to)?;
    let (costs, cell) = shared.group(&place.cell, place.index, &place.level1, installed.group)?;
    Ok((to, costs.cost(&cell, installed.place, place.road)?))
}

struct Search<'a, H> {
    shared: &'a Shared<H>,
    installed: &'a Installed,
    work: &'a mut Workspace,
    limit: usize,
    /// The IDs of the seed cells.
    seeds: Vec<u32>,
    best: u64,
    /// The choices of the end and the start seed of `best`.
    roots: (u8, u8),
    meeting: Option<u32>,
    pops: usize,
    levels: [usize; MAX_LEVELS + 1],
    values: usize,
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
    let clock = clock();
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
        seeds: Vec::new(),
        best: query.ceiling,
        roots: (u8::MAX, u8::MAX),
        meeting: None,
        pops: 0,
        levels: [0; MAX_LEVELS + 1],
        values: 0,
        peak: 0,
    };
    let result = search.run(query);
    let Search { work, pops, levels, values, peak, .. } = search;
    work.count(|stats| {
        stats.searches += 1;
        stats.settled += pops;
        stats.levels.iter_mut().zip(levels).for_each(|(sum, count)| *sum += count);
        stats.values += values;
        stats.peak_bytes = stats.peak_bytes.max(peak);
        stats.search_ns += clock();
    });
    result
}

impl<'a, H: Host> Search<'a, H> {
    fn run(&mut self, query: Query<'_>) -> Result<Option<Found>> {
        let Query { starts, ends, max_steps, cancelled, .. } = query;
        self.peak = self.work.bytes();
        for (side, seeds) in [starts, ends].into_iter().enumerate() {
            for seed in seeds {
                if seed.cost == u64::MAX || !self.shared.has(seed.node) {
                    return Err(Error::InvalidData("Invalid search seed".into()));
                }
                // The cell of the arrival junction of the seed road is a seed cell on both sides,
                // also for a seed that sets no label.
                let cell = self.shared.place(seed.node)?.level1.id;
                if !self.seeds.contains(&cell) {
                    self.seeds.push(cell);
                }
                let label = self.work.labels[side].get(seed.node);
                if (seed.cost, seed.choice) < (label.cost, label.origin) {
                    let label = Label { cost: seed.cost, parent: u32::MAX, origin: seed.choice, level: 0 };
                    self.set(side, seed.node, label)?;
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
        let witness = |side: usize, seeds: &[Seed], road: u32| {
            let label = labels[side].get(road);
            seeds
                .iter()
                .position(|seed| seed.node == road && (seed.cost, seed.choice) == (label.cost, label.origin))
                .ok_or_else(|| Error::InvalidData(format!("Missing {} witness", ["source", "target"][side])))
        };
        // The level of a step is that of the label that it set: the forward label of its road, or
        // the backward label of the road before it.
        let mut path = Vec::new();
        let mut road = meeting;
        loop {
            let label = labels[0].get(road);
            if path.len() > max_steps {
                return Err(Error::Limit);
            }
            path.push(Step { road, level: label.level });
            if label.parent == u32::MAX {
                break;
            }
            road = label.parent;
        }
        path.reverse();
        let source = witness(0, starts, road)?;
        road = meeting;
        loop {
            let label = labels[1].get(road);
            if label.parent == u32::MAX {
                break;
            }
            if path.len() > max_steps {
                return Err(Error::Limit);
            }
            path.push(Step { road: label.parent, level: label.level });
            road = label.parent;
        }
        let target = witness(1, ends, road)?;
        let seeds = starts[source].cost.checked_add(ends[target].cost).ok_or(Error::Limit)?;
        let arcs = self.best.checked_sub(seeds).ok_or_else(|| invalid("a path below its seed costs"))?;
        Ok(Some(Found { cost: self.best, arcs, path, source, target }))
    }

    /// The highest closed cell of a junction of the level-1 cell `home`, with its level: the
    /// highest cell of the junction that holds no seed cell and has a cost table. `None` when each
    /// cell of the junction is open.
    fn closed(&self, home: &Home<'a>) -> Result<Option<(u8, Closed<'a>)>> {
        let differ = self.seeds.iter().map(|&seed| seed ^ home.level1.id).min().unwrap_or(0);
        for level in (1..=self.shared.apart(differ)).rev() {
            if let Some(closed) = self.shared.closed(home, level, self.installed)? {
                return Ok(Some((level, closed)));
            }
        }
        Ok(None)
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
        let result = self.arcs(side, entry.road, &mut arcs).and_then(|level| {
            self.levels[usize::from(level)] += 1;
            for &(road, weight) in &arcs {
                let next = entry.cost.checked_add(weight).filter(|&n| n != u64::MAX).ok_or(Error::Limit)?;
                let label = self.work.labels[side].get(road);
                if next > self.best || (next, entry.origin) >= (label.cost, label.origin) {
                    continue;
                }
                self.set(side, road, Label { cost: next, parent: entry.road, origin: entry.origin, level })?;
                self.meet(road)?;
            }
            Ok(())
        });
        self.work.arcs = arcs;
        result
    }

    /// The arcs of the popped road on its side, and their level. The forward side takes the arrival
    /// junction of the road and gives the arcs `road → v`; the backward side takes its departure
    /// junction and gives the arcs `u → road`. Level 0: the turns at the junction, each with the
    /// cost of the target road of the arc plus the turn cost. A higher level: the finite values of
    /// the table of the highest closed cell of the junction, forward the row of the road to the
    /// leaving roads, backward its column to the entering roads. A road in a download cell that is
    /// not installed has no arc.
    fn arcs(&mut self, side: usize, road: u32, arcs: &mut Vec<(u32, u64)>) -> Result<u8> {
        let (shared, installed) = (self.shared, self.installed);
        let place = shared.place(road)?;
        let mut ids = shared.ids();
        if side == 0 {
            let Some((level, cell)) = self.closed(&place.home())? else {
                let leaving = |k| match ids.get(place.topology.leaving(&place.level1, k)?) {
                    Some(to) => target(shared, installed, to).map(Some),
                    None => Ok(None),
                };
                turns(shared, installed, &place, leaving, arcs)?;
                return Ok(0);
            };
            let row = cell.row(place.reference())?;
            for (column, value) in (0..).zip(cell.table.row(row)).filter(|&(_, value)| value != u64::MAX) {
                if let Some(to) = ids.get(cell.leaving(column)?) {
                    arcs.push((to, value));
                }
            }
            self.values += cell.table.columns as usize;
            return Ok(level);
        }
        let Some(junction) = shared.junction(&place)? else { return Ok(0) };
        if let Some((level, cell)) = self.closed(&junction.home())? {
            let column = cell.column(place.reference())?;
            for (row, value) in (0..).zip(cell.table.column(column)).filter(|&(_, value)| value != u64::MAX) {
                if let Some(from) = ids.get(cell.entering(row)?) {
                    arcs.push((from, value));
                }
            }
            self.values += cell.table.rows as usize;
            return Ok(level);
        }
        let (group, profile) = (installed.group, installed.place);
        let (costs, cell) = shared.group(&place.cell, place.index, &place.level1, group)?;
        let Some(cost) = costs.cost(&cell, profile, place.road)? else { return Ok(0) };
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
        Ok(0)
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
