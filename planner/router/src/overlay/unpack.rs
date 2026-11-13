//! Unpacks the paths of the chosen legs into their roads, level by level. Before each level it
//! announces what the steps of that level read. Then it replaces each step of that level by a
//! search inside the cell of the step, which finds a path of exactly the table value: over the
//! turns of the cell for level 1, else over the tables of its children.

use super::{clock, invalid, target, turns, Entry, Found, Label, Step, Workspace};
use crate::{
    host::Host,
    region::{Installed, Place, Shared},
    router::Control,
    Error, Result,
};

struct Unpack<'a, H> {
    shared: &'a Shared<H>,
    installed: &'a Installed,
    work: &'a mut Workspace,
    control: &'a Control<'a>,
    limit: usize,
    pops: usize,
}

/// The roads of each leg of one route. More than `control.max_roads` roads is a `Limit`. A step
/// whose cell does not give its table value, or a leg whose turns do not cost what its search
/// found, is `InvalidData`.
pub(crate) fn unpack<H: Host>(
    shared: &Shared<H>,
    work: &mut Workspace,
    installed: &Installed,
    legs: &[&Found],
    control: &Control<'_>,
    limit: usize,
) -> Result<Vec<Vec<u32>>> {
    let clock = clock();
    let mut unpack = Unpack { shared, installed, work, control, limit, pops: 0 };
    let result = unpack.run(legs);
    let Unpack { work, pops, .. } = unpack;
    work.count(|stats| {
        stats.unpack_settled += pops;
        stats.unpack_ns += clock();
        for leg in result.iter().flatten() {
            stats.legs += 1;
            stats.roads += leg.len();
        }
    });
    result
}

impl<H: Host> Unpack<'_, H> {
    fn run(&mut self, legs: &[&Found]) -> Result<Vec<Vec<u32>>> {
        let (shared, installed) = (self.shared, self.installed);
        let mut paths: Vec<Vec<Step>> = legs.iter().map(|leg| leg.path.clone()).collect();
        let mut count: usize = paths.iter().map(Vec::len).sum();
        let top = paths.iter().flatten().map(|step| step.level).max().unwrap_or(0);
        for level in (1..=top).rev() {
            let mut spans = Vec::new();
            for pair in paths.iter().flat_map(|path| path.windows(2)).filter(|pair| pair[1].level == level) {
                shared.spans(&shared.place(pair[0].road)?.home(), level, installed, &mut spans)?;
            }
            shared.announce(spans);
            for path in &mut paths {
                let mut next = Vec::with_capacity(path.len());
                for (k, &step) in path.iter().enumerate() {
                    if step.level != level {
                        next.push(step);
                        continue;
                    }
                    if (self.control.cancelled)() {
                        return Err(Error::Cancelled);
                    }
                    let from = k.checked_sub(1).ok_or_else(|| invalid("a path that starts with a table step"))?;
                    let before = next.len();
                    self.cross(path[from].road, step.road, level, &mut next)?;
                    count += next.len() - before - 1;
                    if count > self.control.max_roads {
                        return Err(Error::Limit);
                    }
                }
                *path = next;
            }
        }
        if count > self.control.max_roads {
            return Err(Error::Limit);
        }
        for (leg, path) in legs.iter().zip(&paths) {
            self.check(leg, path)?;
        }
        Ok(paths.into_iter().map(|path| path.into_iter().map(|step| step.road).collect()).collect())
    }

    /// Adds the steps of the level below `level` that lead from road `a` to road `b` across the
    /// cell of `level` that `a` enters: a search of one side from `a`, which relaxes only `b` and
    /// the roads that arrive in the cell, and stops when it settles `b`.
    fn cross(&mut self, a: u32, b: u32, level: u8, steps: &mut Vec<Step>) -> Result<()> {
        let (shared, installed) = (self.shared, self.installed);
        let start = shared.place(a)?;
        let home = start.home();
        let cell = shared.closed(&home, level, installed)?.ok_or_else(|| invalid("a path step without its table"))?;
        let column = cell.column(shared.place(b)?.reference())?;
        let best = cell.table.get(cell.row(start.reference())?, column);
        // Level 1: the cost of `b`, which no block of this cell holds. Higher: the roads of the cell.
        let (end, inside) = match level {
            1 => (target(shared, installed, b)?, 0..0),
            _ => ((b, None), shared.roads(&home, level)?),
        };
        self.work.labels[0].reset()?;
        self.work.queues[0].clear();
        self.set(a, Label { cost: 0, parent: u32::MAX, origin: 0, level: 0 })?;
        let mut arcs = std::mem::take(&mut self.work.arcs);
        let mut ids = shared.ids();
        loop {
            let room = self.limit.saturating_sub(self.work.labels[0].bytes());
            let entry = self.work.queues[0].front(room)?.ok_or_else(|| invalid("a table value without its path"))?;
            self.work.queues[0].pop();
            if self.work.labels[0].get(entry.road).cost != entry.cost {
                continue;
            }
            self.pops += 1;
            if self.pops.is_multiple_of(1024) && (self.control.cancelled)() {
                return Err(Error::Cancelled);
            }
            if entry.road == b {
                break;
            }
            arcs.clear();
            if level == 1 {
                let road = entry.road - (home.cell.first_road + home.level1.first_road);
                let place = Place { road, ..start };
                turns(shared, installed, &place, |k| Ok((k == column).then_some(end)), &mut arcs)?;
            } else {
                let place = shared.place(entry.road)?;
                let child = shared.closed(&place.home(), level - 1, installed)?;
                let child = child.ok_or_else(|| invalid("a cell without the table of its child"))?;
                let row = child.row(place.reference())?;
                for (column, value) in (0..).zip(child.table.row(row)).filter(|&(_, value)| value != u64::MAX) {
                    if let Some(to) = ids.get(child.leaving(column)?).filter(|to| *to == b || inside.contains(to)) {
                        arcs.push((to, value));
                    }
                }
            }
            for &(to, weight) in &arcs {
                let next = entry.cost.checked_add(weight).ok_or(Error::Limit)?;
                if next <= best && next < self.work.labels[0].get(to).cost {
                    self.set(to, Label { cost: next, parent: entry.road, origin: 0, level: level - 1 })?;
                }
            }
        }
        self.work.arcs = arcs;
        if self.work.labels[0].get(b).cost != best {
            return Err(invalid("a path in a cell that does not cost its table value"));
        }
        let first = steps.len();
        let mut road = b;
        while road != a {
            steps.push(Step { road, level: level - 1 });
            road = self.work.labels[0].get(road).parent;
        }
        steps[first..].reverse();
        Ok(())
    }

    fn set(&mut self, road: u32, label: Label) -> Result<()> {
        let room = self.limit.saturating_sub(self.work.queues[0].bytes());
        self.work.labels[0].set(road, label, room)?;
        let room = self.limit.saturating_sub(self.work.labels[0].bytes());
        self.work.queues[0].push(Entry { cost: label.cost, road, origin: label.origin }, room)
    }

    /// The turns of the roads of `path` must cost what the search of `leg` found for its steps.
    fn check(&mut self, leg: &Found, path: &[Step]) -> Result<()> {
        let (shared, installed) = (self.shared, self.installed);
        let mut arcs = std::mem::take(&mut self.work.arcs);
        let mut from = shared.place(path.first().ok_or_else(|| invalid("an empty path"))?.road)?;
        let mut sum = 0u64;
        for (k, step) in path.iter().enumerate().skip(1) {
            if k.is_multiple_of(1024) && (self.control.cancelled)() {
                return Err(Error::Cancelled);
            }
            let to = shared.place(step.road)?;
            arcs.clear();
            let leaving = |k| match from.topology.leaving(&from.level1, k)? == to.reference() {
                true => target(shared, installed, step.road).map(Some),
                false => Ok(None),
            };
            turns(shared, installed, &from, leaving, &mut arcs)?;
            let weight = arcs.iter().filter(|arc| arc.0 == step.road).map(|arc| arc.1).min();
            let weight = weight.ok_or_else(|| Error::InvalidData("A path road without its turn".into()))?;
            sum = sum.checked_add(weight).ok_or(Error::Limit)?;
            from = to;
        }
        self.work.arcs = arcs;
        if sum != leg.arcs {
            return Err(Error::InvalidData("The roads of a path do not cost what its search found".into()));
        }
        Ok(())
    }
}
