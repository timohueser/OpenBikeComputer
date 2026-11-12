//! Divides the junctions into nested cells by recursive bisection with inertial flow, and numbers
//! the junctions in cell order. Each cell of each level is then one range of junction IDs and one
//! range of road IDs.

use crate::column::{Column, Layout};
use crate::{sort, work, Roads};
use planner_router::model::Point;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering::Relaxed};

/// The largest number of levels: a road keeps its border level in a byte of its order key.
pub const MAX_LEVELS: usize = 8;

/// Fails on `cell_sizes` that the partition cannot use: 1 to 8 increasing values, the first from 2
/// and below 2^24, the bits of a junction's place in its level-1 cell in the order key.
pub fn check_sizes(sizes: &[u32]) -> Result<(), String> {
    if sizes.is_empty() || sizes.len() > MAX_LEVELS {
        return Err(format!("`cell_sizes` has {} values; it needs 1 to {MAX_LEVELS}", sizes.len()));
    }
    if sizes[0] < 2 || sizes[0] >= 1 << 24 {
        return Err(format!("`cell_sizes` starts with {}; it needs 2 to 2^24 - 1", sizes[0]));
    }
    if !sizes.is_sorted_by(|a, b| a < b) {
        return Err("the values of `cell_sizes` do not increase".into());
    }
    Ok(())
}

/// The partition of the junctions of walk 1.
pub struct Partition {
    /// The walk-1 ID of each final junction ID.
    pub order: Column<u32>,
    /// The final ID of each walk-1 ID.
    pub rank: Column<u32>,
    pub cells: Cells,
}

/// The cells of the partition. `border`, `leaving_first` and `leaving` are empty until `finish`.
///
/// Rules that the later steps read:
/// 1. Each cell of each level is one range of junction IDs and one range of road IDs. A road is in
///    the cell of its arrival junction.
/// 2. In each level-1 cell the roads are in descending `border`. The entering border roads of a
///    level-ℓ cell are, in each of its level-1 cells, the first roads with `border` ≥ ℓ.
/// 3. The leaving list of a cell holds the roads that depart in the cell and arrive outside it at
///    its level, ascending.
/// 4. The cells of each level are in tree order: ascending ID is the order of the roads.
#[derive(Clone, Debug, Default)]
pub struct Cells {
    /// The first junction and the first road of each level-1 cell; the last entry holds the counts.
    pub level1: Column<[u32; 2]>,
    /// The cell ID of each level-1 cell: from the top level down, the place of each of its cells
    /// among its siblings, in a field of `widths` bits.
    pub ids: Column<u32>,
    /// For each level from 2, the first level-1 cell of each of its cells, then the count of level-1
    /// cells.
    pub upper: Column<u32>,
    /// The field width of each level in a cell ID.
    pub widths: Column<u8>,
    /// The number of cells of each level.
    pub counts: Column<u32>,
    /// The border level of each road: 0 inside one level-1 cell, else the highest level whose
    /// cells of its two junctions differ.
    pub border: Column<u8>,
    /// The start of the leaving list of each cell of all levels, level by level, and the end.
    pub leaving_first: Column<u32>,
    pub leaving: Column<u32>,
}

impl Cells {
    pub fn levels(&self) -> usize {
        self.widths.len()
    }

    /// The level-1 cell of a final junction ID.
    pub fn cell(&self, junction: u32) -> usize {
        self.level1.values().partition_point(|first| first[0] <= junction) - 1
    }

    /// The level-`level` cell, numbered in its level, of level-1 cell `cell`.
    pub fn ancestor(&self, level: usize, cell: usize) -> usize {
        if level == 1 {
            return cell;
        }
        let counts = self.counts.values();
        let start: usize = counts[1..level - 1].iter().map(|&count| count as usize + 1).sum();
        let firsts = &self.upper.values()[start..start + counts[level - 1] as usize];
        firsts.partition_point(|&first| first as usize <= cell) - 1
    }

    /// The number over all levels of cell `cell` of level `level`.
    pub fn global(&self, level: usize, cell: usize) -> usize {
        self.counts.values()[..level - 1].iter().map(|&count| count as usize).sum::<usize>() + cell
    }

    /// The border level of a road between the final junctions `from` and `to`.
    pub fn border(&self, from: u32, to: u32) -> u8 {
        let ids = self.ids.values();
        let differ = ids[self.cell(from)] ^ ids[self.cell(to)];
        if differ == 0 {
            return 0;
        }
        let top = 31 - differ.leading_zeros();
        let mut end = 0;
        for (level, &width) in self.widths.values().iter().enumerate() {
            end += u32::from(width);
            if top < end {
                return level as u8 + 1;
            }
        }
        unreachable!("the fields cover the bits of each ID")
    }

    /// Fills `border` and the leaving lists for the roads in their final order, and checks that each
    /// road lies in the cell of its arrival junction.
    pub fn finish(&mut self, roads: &Roads) -> Result<(), String> {
        let level1 = self.level1.values();
        let (mut border, mut leaving) = (Column::default(), Column::<(u32, u32)>::default());
        for road in 0..roads.len() {
            let (from, to) = (roads.from(road), roads.to(road));
            let cell = self.cell(to);
            if !(level1[cell][1] as usize..level1[cell + 1][1] as usize).contains(&road) {
                return Err(format!("road {road} is not in the cell of its arrival junction"));
            }
            let level = self.border(from, to);
            border.push(level);
            let departure = self.cell(from);
            for level in 1..=usize::from(level) {
                leaving.push((self.global(level, self.ancestor(level, departure)) as u32, road as u32));
            }
        }
        let cells = self.counts.values().iter().map(|&count| count as usize).sum::<usize>();
        let (mut first, mut roads) = (Column::filled(cells + 1, 0u32), Column::default());
        for (cell, road) in sort::sorted(leaving) {
            first.values_mut()[cell as usize + 1] += 1;
            roads.push(road);
        }
        for cell in 0..cells {
            first.values_mut()[cell + 1] += first.values()[cell];
        }
        (self.border, self.leaving_first, self.leaving) = (border, first, roads);
        Ok(())
    }

    /// Prints for each level the number of cells, and the median and largest number for each cell
    /// of its roads, its entering and its leaving border roads.
    pub fn report(&self) {
        let level1 = self.level1.values();
        let border = self.border.values();
        for level in 1..=self.levels() {
            let count = self.counts.values()[level - 1] as usize;
            let (mut roads, mut entering, mut leaving) = (vec![0u32; count], vec![0u32; count], vec![0u32; count]);
            for cell in 0..level1.len() - 1 {
                let range = level1[cell][1] as usize..level1[cell + 1][1] as usize;
                let parent = self.ancestor(level, cell);
                roads[parent] += range.len() as u32;
                entering[parent] += border[range].iter().filter(|&&b| usize::from(b) >= level).count() as u32;
            }
            let first = self.leaving_first.values();
            for (cell, leaving) in leaving.iter_mut().enumerate() {
                let global = self.global(level, cell);
                *leaving = first[global + 1] - first[global];
            }
            let spread = |mut values: Vec<u32>| {
                values.sort_unstable();
                (values[values.len() / 2], values[values.len() - 1])
            };
            let ((rm, rl), (em, el), (lm, ll)) = (spread(roads), spread(entering), spread(leaving));
            eprintln!(
                "Level {level}: {count} cells, {} bits; per cell: roads {rm} (largest {rl}), entering border roads {em} ({el}), leaving {lm} ({ll})",
                self.widths.values()[level - 1]
            );
        }
    }

    pub(crate) fn save(self, layout: &mut Layout) -> Result<(), String> {
        layout.save("cells.level1", self.level1)?;
        layout.save("cells.ids", self.ids)?;
        layout.save("cells.upper", self.upper)?;
        layout.save("cells.widths", self.widths)?;
        layout.save("cells.counts", self.counts)?;
        layout.save("cells.border", self.border)?;
        layout.save("cells.leaving_first", self.leaving_first)?;
        layout.save("cells.leaving", self.leaving)
    }

    pub(crate) fn open(layout: &Layout) -> Result<Self, String> {
        Ok(Cells {
            level1: layout.open("cells.level1")?,
            ids: layout.open("cells.ids")?,
            upper: layout.open("cells.upper")?,
            widths: layout.open("cells.widths")?,
            counts: layout.open("cells.counts")?,
            border: layout.open("cells.border")?,
            leaving_first: layout.open("cells.leaving_first")?,
            leaving: layout.open("cells.leaving")?,
        })
    }
}

/// The junction graph in RAM, with its own junction numbers in Morton order of the points, so that
/// the junctions of a part are near in memory. The order keys, their ties, the islands and the
/// order of a split use the walk-1 IDs, so these numbers change only the speed. The arrays never
/// move: a part of a bisection is a range of positions, and an arc to a junction outside the part
/// is skipped. Parts that run at the same time
/// touch disjoint entries of `residual` and `pos`, so relaxed atomics suffice. A read of `pos` of a
/// junction of another part gives a position in that part, before or after its split: outside.
struct Junctions {
    /// The walk-1 ID of each junction.
    walk1: Vec<u32>,
    /// Longitude and latitude in microdegrees.
    point: Vec<[i32; 2]>,
    /// The first arc of each junction, and the end.
    first: Vec<u32>,
    /// The neighbour of each arc. The arcs of a junction are sorted by neighbour.
    adj: Vec<u32>,
    /// The capacity of each arc, the same in both directions: the roads between the two junctions,
    /// at most 127, so that a residual of up to twice the capacity fits a byte.
    capacity: Vec<u8>,
    residual: Vec<AtomicU8>,
    /// The position of each junction.
    pos: Vec<AtomicU32>,
}

/// The level of a junction that no search reached.
const UNSEEN: u32 = (1 << 30) - 1;
/// The level of a junction on the sink side after a flow.
const SINK_SIDE: u32 = UNSEEN - 1;
/// Above each level of a phase of the flow, and below the levels of the sides (`ISLAND`,
/// `SINK_SIDE`, `UNSEEN`). Small in tests, so that their flows reset the levels; a part of
/// 200 junctions or more then resets them at each phase.
const PHASE_END: u32 = if cfg!(test) { 200 } else { UNSEEN - 3 };
const LEVEL: u32 = (1 << 30) - 1;
/// A sink of the flow.
const SINK: u32 = 1 << 30;
/// On the source side of the best cut so far.
const BEST: u32 = 1 << 31;

/// One part of a bisection: the positions `lo..lo + at.len()`, and the work space of each.
struct Part<'a> {
    lo: u32,
    /// The junction at each position.
    at: &'a mut [u32],
    /// The level of a search, with the flags `SINK` and `BEST`.
    level: &'a mut [u32],
    /// The current arc of the flow search.
    current: &'a mut [u32],
    /// The selection of sources and sinks, the search queue, the path of the flow search, and the
    /// space of the split.
    queue: &'a mut [u32],
}

impl Junctions {
    fn arcs(&self, junction: u32) -> std::ops::Range<usize> {
        self.first[junction as usize] as usize..self.first[junction as usize + 1] as usize
    }

    /// The arc from `to` back to `from`.
    fn twin(&self, from: u32, to: u32) -> usize {
        let arcs = self.arcs(to);
        arcs.start + self.adj[arcs.clone()].binary_search(&from).expect("each arc has its twin")
    }

    /// The order key of a junction in direction `direction`, with its walk-1 ID: a strict order.
    fn key(&self, direction: usize, junction: u32) -> (i64, u32) {
        let [lon, lat] = self.point[junction as usize].map(i64::from);
        ([lon, lat, lon + lat, lon - lat][direction], self.walk1[junction as usize])
    }
}

impl Part<'_> {
    /// The place of `junction` in the part, or `None` outside it.
    fn place(&self, graph: &Junctions, junction: u32) -> Option<usize> {
        let place = graph.pos[junction as usize].load(Relaxed).wrapping_sub(self.lo) as usize;
        (place < self.at.len()).then_some(place)
    }

    fn set_level(&mut self, place: usize, level: u32) {
        self.level[place] = (self.level[place] & !LEVEL) | level;
    }

    /// Splits the part at the smallest of the cuts of four directions. Returns the size of the
    /// source side, which comes first in `at`.
    fn bisect(&mut self, graph: &Junctions) -> usize {
        let n = self.at.len();
        let k = (n / 4).max(1);
        self.level.fill(UNSEEN);
        // (cut, smaller side) of the best direction.
        let mut best: Option<(u64, usize)> = None;
        for direction in 0..4 {
            let key = |&junction: &u32| graph.key(direction, junction);
            self.queue.copy_from_slice(self.at);
            self.queue.select_nth_unstable_by_key(n - k, key);
            self.queue[..n - k].select_nth_unstable_by_key(k - 1, key);
            self.queue[k..n - k].select_nth_unstable_by_key(n / 2 - k, key);
            let median = key(&self.queue[n / 2]);
            for place in 0..n {
                self.level[place] &= BEST;
            }
            for i in n - k..n {
                let place = self.place(graph, self.queue[i]).expect("a sink of the part");
                self.level[place] |= SINK;
            }
            for &junction in self.at.iter() {
                for arc in graph.arcs(junction) {
                    graph.residual[arc].store(graph.capacity[arc], Relaxed);
                }
            }
            let Some(cut) = self.flow(graph, k, best.map(|(cut, _)| cut)) else { continue };
            let sources = self.sides(graph, direction, median);
            let smaller = sources.min(n - sources);
            if best.is_none_or(|best| (cut, std::cmp::Reverse(smaller)) < (best.0, std::cmp::Reverse(best.1))) {
                best = Some((cut, smaller));
                for level in self.level.iter_mut() {
                    *level = if *level & LEVEL == SINK_SIDE { *level & !BEST } else { *level | BEST };
                }
            }
        }
        // A stable split of `at`, the source side first.
        let sources = self.level.iter().filter(|&&level| level & BEST != 0).count();
        let (mut source, mut sink) = (0, sources);
        for place in 0..n {
            let slot = if self.level[place] & BEST != 0 { &mut source } else { &mut sink };
            self.queue[*slot] = self.at[place];
            *slot += 1;
        }
        self.at.copy_from_slice(self.queue);
        for (place, &junction) in self.at.iter().enumerate() {
            graph.pos[junction as usize].store(self.lo + place as u32, Relaxed);
        }
        sources
    }

    /// Dinic's maximum flow from the `k` sources at the start of `queue` to the junctions marked
    /// `SINK`. Returns `None` when the flow passes `bound`. Afterwards the levels of the junctions
    /// that the sources reach are below `UNSEEN`. Each maximum flow gives this set. The search of a
    /// phase starts at the sources with an arc to another junction and runs until it reaches no new
    /// junction; it does not expand a sink. It does not stop at the level of the first sink: each
    /// phase would then push flow on the shortest paths only, which costs more phases.
    ///
    /// The sources have level 0. The levels of a phase start at `start`, above those of the phases
    /// before it, so the levels `1..start` count as unseen, and no phase walks its junctions again
    /// to reset them. `start` is at least 2, so a dead end gets level 1 and is unseen in the next
    /// phase. When the levels come near `PHASE_END`, all but the sources go back to 1.
    fn flow(&mut self, graph: &Junctions, k: usize, bound: Option<u64>) -> Option<u64> {
        let n = self.at.len();
        for place in 0..n {
            self.set_level(place, 1);
        }
        for i in 0..k {
            let place = self.place(graph, self.queue[i]).expect("a source of the part");
            self.set_level(place, 0);
        }
        // The border sources first in `queue`: an inner source has only arcs to sources.
        let mut border = 0;
        for i in 0..k {
            let source = self.queue[i];
            let inner = graph
                .arcs(source)
                .all(|arc| self.place(graph, graph.adj[arc]).is_none_or(|other| self.level[other] & LEVEL == 0));
            if !inner {
                self.queue.swap(border, i);
                border += 1;
            }
        }
        let mut flow = 0u64;
        let mut start = 2;
        loop {
            if start as usize + n >= PHASE_END as usize {
                for place in 0..n {
                    if self.level[place] & LEVEL != 0 {
                        self.set_level(place, 1);
                    }
                }
                start = 2;
            }
            let mut tail = k;
            for i in 0..border {
                let source = self.queue[i];
                let place = self.place(graph, source).expect("a source of the part");
                self.current[place] = graph.first[source as usize];
                tail = self.expand(graph, source, tail, start);
            }
            let (mut head, mut reached) = (k, false);
            while head < tail {
                let junction = self.queue[head];
                head += 1;
                if self.level[self.place(graph, junction).expect("a queued junction")] & SINK != 0 {
                    reached = true;
                    continue;
                }
                tail = self.expand(graph, junction, tail, start);
            }
            if !reached {
                for place in 0..n {
                    if (1..start).contains(&(self.level[place] & LEVEL)) {
                        self.set_level(place, UNSEEN);
                    }
                }
                return Some(flow);
            }
            // The highest level of this phase; `blocking` writes its path over the queue.
            let last = self.queue[tail - 1];
            let top = self.level[self.place(graph, last).expect("a queued junction")] & LEVEL;
            for i in 0..border {
                flow += self.blocking(graph, self.queue[i], k, start);
                if bound.is_some_and(|bound| flow > bound) {
                    return None;
                }
            }
            start = top + 1;
        }
    }

    /// The level after that of the junction at `place` in the phase whose levels start at `start`.
    fn next(&self, place: usize, start: u32) -> u32 {
        match self.level[place] & LEVEL {
            0 => start,
            level => level + 1,
        }
    }

    /// Gives the next level to each neighbour of `junction` over an arc with residual that this
    /// phase did not reach, and queues it at `tail`. Returns the new tail.
    fn expand(&mut self, graph: &Junctions, junction: u32, mut tail: usize, start: u32) -> usize {
        let next = self.next(self.place(graph, junction).expect("a junction of the part"), start);
        for arc in graph.arcs(junction) {
            let neighbour = graph.adj[arc];
            let Some(other) = self.place(graph, neighbour) else { continue };
            if graph.residual[arc].load(Relaxed) > 0 && (1..start).contains(&(self.level[other] & LEVEL)) {
                self.set_level(other, next);
                self.current[other] = graph.first[neighbour as usize];
                self.queue[tail] = neighbour;
                tail += 1;
            }
        }
        tail
    }

    /// Pushes flow from `source` along paths of rising level until no path is left. The path after
    /// the source is in `queue[k..]`. Returns the flow.
    fn blocking(&mut self, graph: &Junctions, source: u32, k: usize, start: u32) -> u64 {
        let mut flow = 0;
        let mut depth = 0;
        loop {
            let junction = if depth == 0 { source } else { self.queue[k + depth - 1] };
            let place = self.place(graph, junction).expect("a junction of the path");
            if depth > 0 && self.level[place] & SINK != 0 {
                flow += u64::from(self.augment(graph, source, k, depth));
                depth = 0;
                continue;
            }
            let end = graph.first[junction as usize + 1];
            let next = self.next(place, start);
            let mut found = None;
            while self.current[place] < end {
                let arc = self.current[place] as usize;
                let neighbour = graph.adj[arc];
                if graph.residual[arc].load(Relaxed) > 0 {
                    if let Some(other) = self.place(graph, neighbour).filter(|&o| self.level[o] & LEVEL == next) {
                        found = Some((neighbour, other));
                        break;
                    }
                }
                self.current[place] += 1;
            }
            match found {
                Some((neighbour, _)) => {
                    self.queue[k + depth] = neighbour;
                    depth += 1;
                }
                None => {
                    if depth == 0 {
                        return flow;
                    }
                    // A dead end: no path passes here again in this phase.
                    self.set_level(place, 1);
                    depth -= 1;
                    let parent = if depth == 0 { source } else { self.queue[k + depth - 1] };
                    let parent = self.place(graph, parent).expect("a junction of the path");
                    self.current[parent] += 1;
                }
            }
        }
    }

    /// Pushes the bottleneck along the path from `source` through `queue[k..k + depth]`.
    fn augment(&self, graph: &Junctions, source: u32, k: usize, depth: usize) -> u8 {
        let path = |i: usize| if i == 0 { source } else { self.queue[k + i - 1] };
        let arc = |i: usize| self.current[self.place(graph, path(i)).expect("a junction of the path")] as usize;
        let bottleneck = (0..depth).map(|i| graph.residual[arc(i)].load(Relaxed)).min().expect("a path of one arc");
        for i in 0..depth {
            let arc = arc(i);
            let twin = graph.twin(path(i), graph.adj[arc]);
            graph.residual[arc].fetch_sub(bottleneck, Relaxed);
            graph.residual[twin].fetch_add(bottleneck, Relaxed);
        }
        bottleneck
    }

    /// After a maximum flow: the junctions that the sources reach are the source side; those that
    /// reach a sink without them the sink side. A component with neither goes whole to the source
    /// side when the key of its lowest junction ID is at or below `median`. Gives each junction of
    /// the sink side the level `SINK_SIDE`, and returns the size of the source side.
    fn sides(&mut self, graph: &Junctions, direction: usize, median: (i64, u32)) -> usize {
        let n = self.at.len();
        // The sink side: a search from the sinks over the junctions that the sources do not reach.
        let mut tail = 0;
        for place in 0..n {
            if self.level[place] & SINK != 0 {
                self.set_level(place, SINK_SIDE);
                self.queue[tail] = self.at[place];
                tail += 1;
            }
        }
        self.spread(graph, 0, tail, SINK_SIDE);
        for place in 0..n {
            if self.level[place] & LEVEL != UNSEEN {
                continue;
            }
            // An island: its own search finds its lowest junction ID.
            const ISLAND: u32 = UNSEEN - 2;
            self.set_level(place, ISLAND);
            self.queue[0] = self.at[place];
            let end = self.spread(graph, 0, 1, ISLAND);
            let lowest = *self.queue[..end]
                .iter()
                .min_by_key(|&&junction| graph.walk1[junction as usize])
                .expect("the island holds a junction");
            let side = if graph.key(direction, lowest) <= median { 0 } else { SINK_SIDE };
            for i in 0..end {
                let place = self.place(graph, self.queue[i]).expect("a junction of the island");
                self.set_level(place, side);
            }
        }
        self.level.iter().filter(|&&level| level & LEVEL != SINK_SIDE).count()
    }

    /// A search over all arcs from the junctions `queue[head..tail]` to the unseen junctions, which
    /// get `level`. Returns the end of the queue.
    fn spread(&mut self, graph: &Junctions, mut head: usize, mut tail: usize, level: u32) -> usize {
        while head < tail {
            let junction = self.queue[head];
            head += 1;
            for arc in graph.arcs(junction) {
                let neighbour = graph.adj[arc];
                if let Some(other) = self.place(graph, neighbour).filter(|&o| self.level[o] & LEVEL == UNSEEN) {
                    self.set_level(other, level);
                    self.queue[tail] = neighbour;
                    tail += 1;
                }
            }
        }
        tail
    }
}

/// Splits the part until each piece has at most `size` junctions. Returns the tree: the range of
/// positions of each node, each node before its source side and then its sink side.
fn split(graph: &Junctions, size: usize, mut part: Part) -> Vec<[u32; 2]> {
    let n = part.at.len();
    let node = [part.lo, part.lo + n as u32];
    if n <= size {
        return vec![node];
    }
    let m = part.bisect(graph);
    let Part { lo, at, level, current, queue } = part;
    let (at, level, current, queue) =
        (at.split_at_mut(m), level.split_at_mut(m), current.split_at_mut(m), queue.split_at_mut(m));
    let source = Part { lo, at: at.0, level: level.0, current: current.0, queue: queue.0 };
    let sink = Part { lo: lo + m as u32, at: at.1, level: level.1, current: current.1, queue: queue.1 };
    let (source, sink) = rayon::join(|| split(graph, size, source), || split(graph, size, sink));
    let mut tree = Vec::with_capacity(1 + source.len() + sink.len());
    tree.push(node);
    tree.extend(source);
    tree.extend(sink);
    tree
}

/// The start of each cell of a level: the largest tree nodes with at most `size` junctions.
fn level_starts(tree: &[[u32; 2]], size: u32) -> Vec<u32> {
    let (mut starts, mut covered) = (Vec::new(), 0);
    for &[lo, hi] in tree {
        if hi - lo <= size && lo >= covered {
            starts.push(lo);
            covered = hi;
        }
    }
    starts
}

/// The bits of `value` spread to the even bits.
fn spread(value: u32) -> u64 {
    let mut value = u64::from(value);
    value = (value | value << 16) & 0x0000_ffff_0000_ffff;
    value = (value | value << 8) & 0x00ff_00ff_00ff_00ff;
    value = (value | value << 4) & 0x0f0f_0f0f_0f0f_0f0f;
    value = (value | value << 2) & 0x3333_3333_3333_3333;
    (value | value << 1) & 0x5555_5555_5555_5555
}

/// The bits of `value`.
fn bits(value: u32) -> u8 {
    (32 - value.leading_zeros()) as u8
}

/// Divides the junctions of walk 1, at `points`, with the roads `ends`, into nested cells of at most
/// `sizes` junctions, which `check_sizes` accepted. Fails when the junction graph needs more than
/// the page budget, and when the cell IDs need more than 32 bits.
pub fn partition(points: &Column<Point>, ends: &Column<(u32, u32)>, sizes: &[u32]) -> Result<Partition, String> {
    let started = std::time::Instant::now();
    let j = points.len();
    // Each pair of junctions with a road between them once, with the number of its roads.
    let pairs: Column<(u32, u32)> =
        ends.values().iter().filter(|(from, to)| from != to).map(|&(from, to)| (from.min(to), from.max(to))).collect();
    let edges: Column<((u32, u32), u8)> =
        sort::sorted(pairs).counted().map(|(pair, count)| (pair, count.min(127) as u8)).collect();
    let arcs = 2 * edges.len();
    let need = 36 * j as u64 + 6 * arcs as u64;
    let budget = work::page_budget()?.0;
    if need > budget {
        return Err(format!("The partition needs {need} bytes; the page budget is {budget}"));
    }
    u32::try_from(arcs).map_err(|_| format!("The partition has {arcs} arcs; it allows fewer than 2^32"))?;
    work::Guard::stage_within("partition", need + (256 << 20));
    let morton = |point: &Point| {
        spread(point.lon.wrapping_add(i32::MIN) as u32) | spread(point.lat.wrapping_add(i32::MIN) as u32) << 1
    };
    let keys: Column<(u64, u32)> = points.values().iter().zip(0..).map(|(point, id)| (morton(point), id)).collect();
    let walk1: Vec<u32> = sort::sorted(keys).map(|(_, id)| id).collect();
    // The own number of each walk-1 ID, and the first order of the positions: walk-1 order.
    let mut at = vec![0u32; j];
    for (junction, &id) in walk1.iter().enumerate() {
        at[id as usize] = junction as u32;
    }
    let mut level = vec![0u32; j];
    let mut graph = Junctions {
        point: walk1.iter().map(|&id| points.values()[id as usize]).map(|point| [point.lon, point.lat]).collect(),
        walk1,
        first: vec![0; j + 1],
        adj: vec![0; arcs],
        capacity: vec![0; arcs],
        residual: (0..arcs).map(|_| AtomicU8::new(0)).collect(),
        pos: (0..j as u32).map(|_| AtomicU32::new(0)).collect(),
    };
    for (place, &junction) in at.iter().enumerate() {
        graph.pos[junction as usize] = AtomicU32::new(place as u32);
    }
    for &((a, b), _) in edges.values() {
        graph.first[at[a as usize] as usize + 1] += 1;
        graph.first[at[b as usize] as usize + 1] += 1;
    }
    for i in 0..j {
        graph.first[i + 1] += graph.first[i];
    }
    // `level` is the cursor of each list here.
    level.copy_from_slice(&graph.first[..j]);
    for &((a, b), capacity) in edges.values() {
        let (a, b) = (at[a as usize], at[b as usize]);
        for (from, to) in [(a, b), (b, a)] {
            let arc = level[from as usize] as usize;
            (graph.adj[arc], graph.capacity[arc]) = (to, capacity);
            level[from as usize] += 1;
        }
    }
    drop(edges);
    // Each list sorted by neighbour, for `twin`.
    let mut list = Vec::new();
    for junction in 0..j as u32 {
        let arcs = graph.arcs(junction);
        list.clear();
        list.extend(arcs.clone().map(|arc| (graph.adj[arc], graph.capacity[arc])));
        list.sort_unstable();
        for (arc, &(neighbour, capacity)) in arcs.zip(&list) {
            (graph.adj[arc], graph.capacity[arc]) = (neighbour, capacity);
        }
    }
    let (mut current, mut queue) = (vec![0u32; j], vec![0u32; j]);
    let part = Part { lo: 0, at: &mut at, level: &mut level, current: &mut current, queue: &mut queue };
    let tree = split(&graph, sizes[0] as usize, part);
    drop((level, current, queue));
    let mut rank = Column::filled(j, 0u32);
    for (junction, pos) in graph.pos.iter().enumerate() {
        rank.values_mut()[graph.walk1[junction] as usize] = pos.load(Relaxed);
    }
    let order: Column<u32> = at.iter().map(|&junction| graph.walk1[junction as usize]).collect();
    drop(graph);

    let mut starts = vec![level_starts(&tree, sizes[0])];
    for &size in &sizes[1..] {
        let cells = level_starts(&tree, size);
        if cells.len() < 2 {
            break;
        }
        starts.push(cells);
    }
    let cells = cells(&starts, j as u32, ends, &rank)?;
    let mut hash = Sha256::new();
    let columns = [order.values(), cells.ids.values(), cells.upper.values(), cells.counts.values()];
    for value in columns.into_iter().flatten().chain(cells.level1.values().iter().flatten()) {
        hash.update(value.to_le_bytes());
    }
    hash.update(cells.widths.values());
    eprintln!(
        "Partition: {j} junctions, {arcs} arcs, need {} MiB, {:.1} s, {} levels, SHA-256 {:x}",
        need >> 20,
        started.elapsed().as_secs_f64(),
        starts.len(),
        hash.finalize()
    );
    Ok(Partition { order, rank, cells })
}

/// The cells of the levels whose cells start at `starts`, and the roads of each level-1 cell: those
/// of `ends` whose arrival junction has its final ID there.
fn cells(starts: &[Vec<u32>], junctions: u32, ends: &Column<(u32, u32)>, rank: &Column<u32>) -> Result<Cells, String> {
    let levels = starts.len();
    let ancestor = |level: usize, start: u32| starts[level].partition_point(|&first| first <= start) - 1;
    // The field of a level holds the place of a cell among the cells of its parent.
    let mut widths = Vec::new();
    for level in 0..levels {
        let largest = if level + 1 == levels {
            starts[level].len() as u32
        } else {
            let mut children = vec![0u32; starts[level + 1].len()];
            for &start in &starts[level] {
                children[ancestor(level + 1, start)] += 1;
            }
            children.into_iter().max().unwrap_or(1)
        };
        widths.push(bits(largest.saturating_sub(1)));
    }
    let total: u32 = widths.iter().map(|&width| u32::from(width)).sum();
    if total > 32 {
        let fields: Vec<String> =
            widths.iter().enumerate().map(|(level, width)| format!("level {}: {width}", level + 1)).collect();
        return Err(format!(
            "The cell IDs need {total} bits ({}); at most 32. Remove a level from `cell_sizes`",
            fields.join(", ")
        ));
    }
    let mut ids = Column::default();
    for &start in &starts[0] {
        let (mut id, mut shift) = (0u32, 0);
        for level in 0..levels {
            let cell = ancestor(level, start);
            let first =
                if level + 1 == levels { 0 } else { ancestor(level, starts[level + 1][ancestor(level + 1, start)]) };
            id |= ((cell - first) as u32).checked_shl(shift).unwrap_or(0);
            shift += u32::from(widths[level]);
        }
        ids.push(id);
    }
    let mut upper = Column::default();
    for level in &starts[1..] {
        upper.extend(&level.iter().map(|&start| ancestor(0, start) as u32).collect::<Vec<_>>());
        upper.push(starts[0].len() as u32);
    }
    let mut roads = vec![0u32; starts[0].len() + 1];
    for &(_, to) in ends.values() {
        roads[ancestor(0, rank.values()[to as usize]) + 1] += 1;
    }
    let mut level1 = Column::default();
    let mut first_road = 0;
    for (cell, &start) in starts[0].iter().chain([&junctions]).enumerate() {
        first_road += roads[cell];
        level1.push([start, first_road]);
    }
    Ok(Cells {
        level1,
        ids,
        upper,
        widths: widths.into_iter().collect(),
        counts: starts.iter().map(|level| level.len() as u32).collect(),
        ..Cells::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::NO_ELEVATION;

    /// A 40 × 40 grid with two roads on each edge, junction `40 × row + column` at that place, and an
    /// island of 3 junctions at column 12.5 and row 20.5, with no road to the grid.
    fn grid() -> (Column<Point>, Vec<(u32, u32)>) {
        let point = |column: f64, row: f64| Point {
            lon: (column * 1000.0) as i32,
            lat: (row * 1000.0) as i32,
            elevation: NO_ELEVATION,
        };
        let mut points: Vec<_> = (0..1600).map(|i| point(f64::from(i % 40), f64::from(i / 40))).collect();
        points.extend([point(12.5, 20.5), point(12.6, 20.5), point(12.5, 20.6)]);
        let mut ends = Vec::new();
        for i in 0..1600u32 {
            for next in [i + 1, i + 40].into_iter().filter(|&next| next < 1600 && (next == i + 40 || i % 40 != 39)) {
                ends.extend([(i, next), (next, i)]);
            }
        }
        ends.extend([(1600, 1601), (1601, 1602), (1602, 1600)]);
        (points.into_iter().collect(), ends)
    }

    /// The final IDs of each cell of each level, as walk-1 IDs, and checks that each is one range
    /// within its size.
    fn check(partition: &Partition, sizes: &[u32]) {
        let cells = &partition.cells;
        let level1 = cells.level1.values();
        assert_eq!(level1.last().unwrap()[0] as usize, partition.order.len());
        for level in 1..=cells.levels() {
            for cell in 0..cells.counts.values()[level - 1] as usize {
                let members: Vec<usize> = (0..level1.len() - 1).filter(|&c| cells.ancestor(level, c) == cell).collect();
                assert!(
                    members.windows(2).all(|pair| pair[1] == pair[0] + 1),
                    "level {level} cell {cell} is one range"
                );
                let size = level1[members[members.len() - 1] + 1][0] - level1[members[0]][0];
                assert!(size <= sizes[level - 1], "level {level} cell {cell} has {size} junctions");
            }
        }
        for (new, &old) in partition.order.values().iter().enumerate() {
            assert_eq!(partition.rank.values()[old as usize] as usize, new);
        }
    }

    #[test]
    fn a_grid_is_cut_at_its_minimum_into_nested_ranges() {
        let (points, ends) = grid();
        let ends: Column<_> = ends.into_iter().collect();
        let sizes = [16, 64, 256];
        let partition = partition(&points, &ends, &sizes).unwrap();
        check(&partition, &sizes);
        assert_eq!(partition.cells.levels(), 3);
        // The top cut: two roads on each of 40 edges between columns 9 and 10. The source side, the
        // first positions, holds columns 0 to 9 and the island.
        let mut expected: Vec<u32> = (0..1600).filter(|i| i % 40 < 10).chain(1600..1603).collect();
        let crossing =
            |side: &[u32]| ends.values().iter().filter(|(a, b)| side.contains(a) != side.contains(b)).count();
        assert_eq!(crossing(&expected), 80);
        let mut found = partition.order.values()[..expected.len()].to_vec();
        found.sort_unstable();
        expected.sort_unstable();
        assert_eq!(found, expected);
    }

    #[test]
    fn one_and_four_threads_give_the_same_cells() {
        let (points, mut ends) = grid();
        // Parallel roads, and a junction of degree 60.
        ends.extend([(5, 6), (5, 6), (6, 5)]);
        ends.extend((0..60).map(|i| (820, i * 26)));
        let ends: Column<_> = ends.into_iter().collect();
        let sizes = [16, 64, 256];
        let run = |threads| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let partition = pool.install(|| partition(&points, &ends, &sizes)).unwrap();
            check(&partition, &sizes);
            let c = &partition.cells;
            let level1: Vec<_> = c.level1.values().to_vec();
            (partition.order.values().to_vec(), level1, c.ids.values().to_vec(), c.upper.values().to_vec(), {
                (c.widths.values().to_vec(), c.counts.values().to_vec())
            })
        };
        assert_eq!(run(1), run(4));
    }

    /// Junctions `0..n` in their own order, with the roads `(a, b, capacity)`, each pair once.
    fn junctions(n: u32, roads: &[(u32, u32, u8)]) -> Junctions {
        let mut lists = vec![Vec::new(); n as usize];
        for &(a, b, capacity) in roads {
            lists[a as usize].push((b, capacity));
            lists[b as usize].push((a, capacity));
        }
        let (mut first, mut adj, mut capacity) = (vec![0], Vec::new(), Vec::new());
        for list in &mut lists {
            list.sort_unstable();
            adj.extend(list.iter().map(|&(b, _)| b));
            capacity.extend(list.iter().map(|&(_, c)| c));
            first.push(adj.len() as u32);
        }
        Junctions {
            walk1: (0..n).collect(),
            point: vec![[0, 0]; n as usize],
            first,
            adj,
            residual: capacity.iter().map(|&c| AtomicU8::new(c)).collect(),
            capacity,
            pos: (0..n).map(AtomicU32::new).collect(),
        }
    }

    /// The maximum flow from `sources` to `sinks` by shortest augmenting paths, and the junctions
    /// that the sources reach after it.
    fn plain_flow(n: usize, roads: &[(u32, u32, u8)], sources: &[u32], sinks: &[u32]) -> (u64, Vec<bool>) {
        let mut residual = vec![vec![0i32; n]; n];
        for &(a, b, capacity) in roads {
            residual[a as usize][b as usize] += i32::from(capacity);
            residual[b as usize][a as usize] += i32::from(capacity);
        }
        let mut flow = 0;
        loop {
            let mut parent = vec![usize::MAX; n];
            let mut queue: std::collections::VecDeque<usize> = sources.iter().map(|&s| s as usize).collect();
            for &source in sources {
                parent[source as usize] = source as usize;
            }
            let mut sink = None;
            while let Some(u) = queue.pop_front() {
                if sinks.contains(&(u as u32)) {
                    sink = Some(u);
                    break;
                }
                for v in 0..n {
                    if residual[u][v] > 0 && parent[v] == usize::MAX {
                        parent[v] = u;
                        queue.push_back(v);
                    }
                }
            }
            let Some(sink) = sink else {
                return (flow, parent.iter().map(|&p| p != usize::MAX).collect());
            };
            let mut path = vec![sink];
            while parent[path[path.len() - 1]] != path[path.len() - 1] {
                path.push(parent[path[path.len() - 1]]);
            }
            let bottleneck = path.windows(2).map(|w| residual[w[1]][w[0]]).min().unwrap();
            for w in path.windows(2) {
                residual[w[1]][w[0]] -= bottleneck;
                residual[w[0]][w[1]] += bottleneck;
            }
            flow += bottleneck as u64;
        }
    }

    #[test]
    fn the_flow_gives_the_cut_of_a_plain_maximum_flow() {
        let mut seed = 7u64;
        let mut random = |below: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as u32 % below
        };
        for round in 0..300 {
            // A 7 × 7 grid with some roads missing (dead ends and islands), parallel roads, and some
            // roads made chains of junctions with unequal capacities. Every 3rd graph has
            // capacity 1 everywhere, so that many cuts are minimal.
            let mut n = 49;
            let mut roads = Vec::new();
            for i in 0..49u32 {
                for next in [i + 1, i + 7].into_iter().filter(|&next| next < 49 && (next == i + 7 || i % 7 != 6)) {
                    if random(6) == 0 {
                        continue;
                    }
                    let capacity =
                        |random: &mut dyn FnMut(u32) -> u32| if round % 3 == 0 { 1 } else { 1 + random(3) as u8 };
                    let mut from = i;
                    for _ in 0..[0, 0, 1, 3][random(4) as usize] {
                        roads.push((from, n, capacity(&mut random)));
                        (from, n) = (n, n + 1);
                    }
                    roads.push((from, next, capacity(&mut random)));
                }
            }
            let sources: Vec<u32> = (0..49).filter(|i| i % 7 < 2).collect();
            let sinks: Vec<u32> = (0..49).filter(|i| i % 7 > 4).collect();
            let (expected, reached) = plain_flow(n as usize, &roads, &sources, &sinks);

            let graph = junctions(n, &roads);
            let (mut at, mut level) = ((0..n).collect::<Vec<_>>(), vec![0u32; n as usize]);
            let (mut current, mut queue) = (vec![0u32; n as usize], vec![0u32; n as usize]);
            queue[..sources.len()].copy_from_slice(&sources);
            for &sink in &sinks {
                level[sink as usize] |= SINK;
            }
            let mut part = Part { lo: 0, at: &mut at, level: &mut level, current: &mut current, queue: &mut queue };
            let flow = part.flow(&graph, sources.len(), None);
            assert_eq!(flow, Some(expected), "round {round}");
            let found: Vec<bool> = level.iter().map(|&level| level & LEVEL != UNSEEN).collect();
            assert_eq!(found, reached, "round {round}");
        }
    }

    #[test]
    fn a_part_of_three_junctions_splits() {
        let points: Column<Point> = (0..3).map(|i| Point { lon: i * 1000, lat: 0, elevation: NO_ELEVATION }).collect();
        let ends: Column<_> = [(0, 1), (1, 0), (1, 2), (2, 1)].into_iter().collect();
        let partition = partition(&points, &ends, &[2]).unwrap();
        check(&partition, &[2]);
        assert_eq!(partition.cells.counts.values(), [2]);
    }
}
