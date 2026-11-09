use crate::{base::group, column::Column};
use planner_router::landmarks;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
};

const LANDMARKS: usize = 6;

pub struct Junctions {
    from: Column<u32>,
    to: Column<u32>,
    /// The roads that leave (side 0) and reach (side 1) junction `j` are
    /// `roads[side][first[side][j]..first[side][j + 1]]`.
    first: [Column<u32>; 2],
    roads: [Column<u32>; 2],
}

impl Junctions {
    /// The endpoints are graph point indices, dense from 0: the mapping takes one slot for each
    /// index up to the largest.
    pub fn new(endpoints: impl Iterator<Item = (u32, u32)> + Clone) -> Result<Self, String> {
        let points = endpoints.clone().map(|(from, to)| from.max(to) as usize + 1).max().unwrap_or(0);
        let mut mapping = Column::filled(points, u32::MAX);
        let mapping = mapping.values_mut();
        let mut nodes = 0;
        // Arrival order keeps the distance columns close to the road geometry order.
        for node in endpoints.clone().map(|(_, to)| to).chain(endpoints.clone().map(|(from, _)| from)) {
            let junction = &mut mapping[node as usize];
            if *junction == u32::MAX {
                *junction = nodes;
                nodes = nodes.checked_add(1).filter(|&n| n != u32::MAX).ok_or("Too many landmark junctions")?;
            }
        }
        let from: Column<u32> = endpoints.clone().map(|(from, _)| mapping[from as usize]).collect();
        let to: Column<u32> = endpoints.map(|(_, to)| mapping[to as usize]).collect();
        let [leaving, reaching] = [&from, &to].map(|ends| group(ends.values(), nodes as usize));
        Ok(Self { from, to, first: [leaving.0, reaching.0], roads: [leaving.1, reaching.1] })
    }

    pub fn index(&self, write: &mut impl FnMut(&[u8]) -> Result<String, String>) -> Result<landmarks::Index, String> {
        Ok(landmarks::Index {
            junctions: self.first[0].values().len() as u32 - 1,
            mapping: landmarks::write(self.to.values().iter().copied(), write)?,
            profiles: BTreeMap::new(),
        })
    }

    /// The junctions of the largest strongly connected component of the roads with a cost, in
    /// ascending order. Of equal components, the first that Kosaraju finds wins.
    fn component(&self, costs: &[u32]) -> Column<u32> {
        let (first, roads) = (self.first.each_ref().map(Column::values), self.roads.each_ref().map(Column::values));
        let (from, to) = (self.from.values(), self.to.values());
        let nodes = first[0].len() - 1;
        let mut seen = Column::filled(nodes, false);
        let seen = seen.values_mut();
        let mut order = Column::default();
        let mut stack = Vec::new();
        for root in 0..nodes as u32 {
            if seen[root as usize] {
                continue;
            }
            seen[root as usize] = true;
            stack.push((root, first[0][root as usize]));
            while let Some(&(node, arc)) = stack.last() {
                if arc == first[0][node as usize + 1] {
                    stack.pop();
                    order.push(node);
                } else {
                    stack.last_mut().unwrap().1 += 1;
                    let road = roads[0][arc as usize] as usize;
                    let next = to[road];
                    if costs[road] != u32::MAX && !seen[next as usize] {
                        seen[next as usize] = true;
                        stack.push((next, first[0][next as usize]));
                    }
                }
            }
        }
        // The component of each junction; `u32::MAX` until the reverse search reaches it.
        let mut components = Column::filled(nodes, u32::MAX);
        let components = components.values_mut();
        let (mut largest, mut largest_size) = (u32::MAX, 0);
        let mut pending = Vec::new();
        for (id, &root) in (0u32..).zip(order.values().iter().rev()) {
            if components[root as usize] != u32::MAX {
                continue;
            }
            components[root as usize] = id;
            pending.push(root);
            let mut size = 0;
            while let Some(node) = pending.pop() {
                size += 1;
                for arc in first[1][node as usize]..first[1][node as usize + 1] {
                    let road = roads[1][arc as usize] as usize;
                    let previous = from[road];
                    if costs[road] != u32::MAX && components[previous as usize] == u32::MAX {
                        components[previous as usize] = id;
                        pending.push(previous);
                    }
                }
            }
            if size > largest_size {
                (largest, largest_size) = (id, size);
            }
        }
        (0..nodes as u32).filter(|&node| components[node as usize] == largest).collect()
    }

    /// The cost from every junction to `source`, or with `forward` from `source` to every
    /// junction, with each road cost divided by `scale` first. Rounding arc costs down preserves
    /// feasibility; distance rounding alone does not.
    fn distances(&self, costs: &[u32], source: u32, scale: u64, forward: bool) -> Column<u64> {
        let (first, roads) = (self.first.each_ref().map(Column::values), self.roads.each_ref().map(Column::values));
        let (from, to) = (self.from.values(), self.to.values());
        let side = usize::from(!forward);
        let mut column = Column::filled(first[0].len() - 1, u64::MAX);
        let distances = column.values_mut();
        let mut queue = BinaryHeap::new();
        distances[source as usize] = 0;
        queue.push(Reverse((0u64, source)));
        while let Some(Reverse((cost, node))) = queue.pop() {
            if cost != distances[node as usize] {
                continue;
            }
            for arc in first[side][node as usize]..first[side][node as usize + 1] {
                let road = roads[side][arc as usize] as usize;
                if costs[road] == u32::MAX {
                    continue;
                }
                let beyond = if forward { to[road] } else { from[road] } as usize;
                let next = cost + costs[road] as u64 / scale;
                if next < distances[beyond] {
                    distances[beyond] = next;
                    queue.push(Reverse((next, beyond as u32)));
                }
            }
        }
        column
    }

    pub fn prepare(
        &self,
        costs: &[u32],
        write: &mut impl FnMut(&[u8]) -> Result<String, String>,
    ) -> Result<landmarks::Columns, String> {
        let (first, roads) = (self.first.each_ref().map(Column::values), self.roads.each_ref().map(Column::values));
        let component = self.component(costs);
        let component = component.values();
        let mut next = component
            .iter()
            .copied()
            .find(|&node| {
                (0..2).all(|side| {
                    roads[side][first[side][node as usize] as usize..first[side][node as usize + 1] as usize]
                        .iter()
                        .filter(|&&road| costs[road as usize] != u32::MAX)
                        .take(2)
                        .count()
                        == 2
                })
            })
            .or_else(|| component.first().copied())
            .ok_or("Landmark graph is empty")?;
        // A column that reaches the cap is flat there and guides nothing. The cost from any
        // junction to any landmark is at most its cost to the first landmark plus the cost from
        // there, so this scale keeps every column of the component below the cap.
        let farthest = |forward| {
            self.distances(costs, next, 1, forward)
                .values()
                .iter()
                .copied()
                .filter(|&d| d != u64::MAX)
                .max()
                .unwrap_or(0)
        };
        let span = farthest(false).saturating_add(farthest(true));
        let scale = u32::try_from(span.div_ceil(u16::MAX as u64 - 1).max(1))
            .map_err(|_| "Landmark distances exceed the scale range")?;
        let mut nearest = Column::filled(first[0].len() - 1, u64::MAX);
        let nearest = nearest.values_mut();
        let mut tables = Vec::new();
        for _ in 0..LANDMARKS {
            let distances = self.distances(costs, next, scale as u64, false);
            let distances = distances.values();
            for &node in component {
                nearest[node as usize] = nearest[node as usize].min(distances[node as usize]);
            }
            next = *component.iter().max_by_key(|&&node| nearest[node as usize]).unwrap();
            tables.push(landmarks::write(distances.iter().map(|&d| d.min(u16::MAX as u64) as u32), write)?);
            if nearest[next as usize] == 0 {
                break;
            }
        }
        Ok(landmarks::Columns { scale, tables })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::table::Table;

    #[test]
    fn compressed_bounds_stay_feasible_with_unused_point_indices_and_a_scale_for_the_region() {
        let endpoints = [(10, 20), (20, 30), (30, 10), (30, 40), (40, 30), (2_000, 2_001)];
        let graph = Junctions::new(endpoints.into_iter()).unwrap();
        let mut objects = BTreeMap::new();
        let mut write = |bytes: &[u8]| {
            let key = planner_router::package::digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        };
        let mut index = graph.index(&mut write).unwrap();
        let metrics = [vec![9, 15, 8, u32::MAX - 17, u32::MAX - 8, 9], vec![u32::MAX, 15, 8, 9, 17, 9]];
        for (i, costs) in metrics.iter().enumerate() {
            index.profiles.insert(i.to_string(), graph.prepare(costs, &mut write).unwrap());
        }
        assert!(index.valid(endpoints.len() as u32));
        let read = |table: &Table| -> Vec<u32> {
            table
                .blocks
                .iter()
                .flat_map(|key| {
                    let deltas: Vec<i64> = planner_router::storage::decode(&objects[key]).unwrap();
                    let mut value = 0i64;
                    deltas
                        .into_iter()
                        .map(|d| {
                            value += d;
                            u32::try_from(value).unwrap()
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        assert_eq!(read(&index.mapping), graph.to.values());
        // The far junction of the costly roads is a landmark, and the scale keeps it below the cap.
        assert!(index.profiles["0"].tables.iter().any(|table| read(table)[graph.to.values()[3] as usize] == 0));
        assert!(index.profiles["0"].scale > 1);
        assert_eq!(index.profiles["1"].scale, 1);
        let mut saturated = false;
        for (i, costs) in metrics.iter().enumerate() {
            let columns = &index.profiles[&i.to_string()];
            for table in &columns.tables {
                let distances = read(table);
                assert_eq!(distances.len(), index.junctions as usize);
                saturated |= distances.contains(&(u16::MAX as u32));
                for (road, &cost) in costs.iter().enumerate() {
                    if cost != u32::MAX {
                        assert!(
                            distances[graph.from.values()[road] as usize] as u64 * columns.scale as u64
                                <= cost as u64
                                    + distances[graph.to.values()[road] as usize] as u64 * columns.scale as u64
                        );
                    }
                }
            }
        }
        // Only the junction outside the component stays at the cap.
        assert!(saturated);
        for table in &index.profiles["0"].tables {
            let distances = read(table);
            assert!((0..4).all(|road| distances[graph.from.values()[road] as usize] < u16::MAX as u32));
        }
    }
}
