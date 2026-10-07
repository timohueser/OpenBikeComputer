use planner_router::landmarks;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap, HashMap},
};

const LANDMARKS: usize = 6;

pub struct Junctions {
    from: Vec<u32>,
    to: Vec<u32>,
    first: [Vec<u32>; 2],
    roads: [Vec<u32>; 2],
}

impl Junctions {
    pub fn new(endpoints: impl Iterator<Item = (u32, u32)> + Clone) -> Result<Self, String> {
        let mut mapping = HashMap::new();
        // Arrival order keeps the distance columns close to the road geometry order.
        for node in endpoints.clone().map(|(_, to)| to).chain(endpoints.clone().map(|(from, _)| from)) {
            let next = u32::try_from(mapping.len()).map_err(|_| "Too many landmark junctions")?;
            mapping.entry(node).or_insert(next);
        }
        let nodes = mapping.len();
        let from: Vec<_> = endpoints.clone().map(|(from, _)| mapping[&from]).collect();
        let to: Vec<_> = endpoints.map(|(_, to)| mapping[&to]).collect();
        drop(mapping);
        let mut first = [vec![0u32; nodes + 1], vec![0u32; nodes + 1]];
        let mut roads = [vec![0u32; from.len()], vec![0u32; from.len()]];
        for (side, endpoints) in [&from, &to].into_iter().enumerate() {
            for &node in endpoints {
                first[side][node as usize + 1] += 1;
            }
            for node in 1..first[side].len() {
                first[side][node] += first[side][node - 1];
            }
            let mut cursor = first[side].clone();
            for (road, &node) in endpoints.iter().enumerate() {
                roads[side][cursor[node as usize] as usize] = road as u32;
                cursor[node as usize] += 1;
            }
        }
        Ok(Self { from, to, first, roads })
    }

    pub fn index(&self, write: &mut impl FnMut(&[u8]) -> Result<String, String>) -> Result<landmarks::Index, String> {
        Ok(landmarks::Index {
            junctions: self.first[0].len() as u32 - 1,
            mapping: landmarks::write(self.to.iter().copied(), write)?,
            profiles: BTreeMap::new(),
        })
    }

    fn component(&self, costs: &[u64]) -> Vec<u32> {
        let nodes = self.first[0].len() - 1;
        let mut seen = vec![false; nodes];
        let mut order = Vec::with_capacity(nodes);
        let mut stack = Vec::new();
        for root in 0..nodes as u32 {
            if seen[root as usize] {
                continue;
            }
            seen[root as usize] = true;
            stack.push((root, self.first[0][root as usize]));
            while let Some(&(node, arc)) = stack.last() {
                if arc == self.first[0][node as usize + 1] {
                    stack.pop();
                    order.push(node);
                } else {
                    stack.last_mut().unwrap().1 += 1;
                    let road = self.roads[0][arc as usize] as usize;
                    let to = self.to[road];
                    if costs[road] != u64::MAX && !seen[to as usize] {
                        seen[to as usize] = true;
                        stack.push((to, self.first[0][to as usize]));
                    }
                }
            }
        }
        seen.fill(false);
        let mut pending = Vec::new();
        let mut members = Vec::new();
        let mut largest = Vec::new();
        for root in order.into_iter().rev() {
            if seen[root as usize] {
                continue;
            }
            seen[root as usize] = true;
            pending.push(root);
            members.clear();
            while let Some(node) = pending.pop() {
                members.push(node);
                for arc in self.first[1][node as usize]..self.first[1][node as usize + 1] {
                    let road = self.roads[1][arc as usize] as usize;
                    let from = self.from[road];
                    if costs[road] != u64::MAX && !seen[from as usize] {
                        seen[from as usize] = true;
                        pending.push(from);
                    }
                }
            }
            if members.len() > largest.len() {
                std::mem::swap(&mut largest, &mut members);
            }
        }
        largest.sort_unstable();
        largest
    }

    /// The cost from every junction to `source`, or with `forward` from `source` to every
    /// junction, with each road cost divided by `scale` first. Rounding arc costs down preserves
    /// feasibility; distance rounding alone does not.
    fn distances(&self, costs: &[u64], source: u32, scale: u64, forward: bool) -> Vec<u64> {
        let side = usize::from(!forward);
        let mut distances = vec![u64::MAX; self.first[0].len() - 1];
        let mut queue = BinaryHeap::new();
        distances[source as usize] = 0;
        queue.push(Reverse((0u64, source)));
        while let Some(Reverse((cost, node))) = queue.pop() {
            if cost != distances[node as usize] {
                continue;
            }
            for arc in self.first[side][node as usize]..self.first[side][node as usize + 1] {
                let road = self.roads[side][arc as usize] as usize;
                if costs[road] == u64::MAX {
                    continue;
                }
                let beyond = if forward { self.to[road] } else { self.from[road] } as usize;
                let next = cost + costs[road] / scale;
                if next < distances[beyond] {
                    distances[beyond] = next;
                    queue.push(Reverse((next, beyond as u32)));
                }
            }
        }
        distances
    }

    pub fn prepare(
        &self,
        costs: &[u64],
        write: &mut impl FnMut(&[u8]) -> Result<String, String>,
    ) -> Result<landmarks::Columns, String> {
        let component = self.component(costs);
        let mut next = component
            .iter()
            .copied()
            .find(|&node| {
                (0..2).all(|side| {
                    self.roads[side]
                        [self.first[side][node as usize] as usize..self.first[side][node as usize + 1] as usize]
                        .iter()
                        .filter(|&&road| costs[road as usize] != u64::MAX)
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
        let farthest =
            |forward| self.distances(costs, next, 1, forward).into_iter().filter(|&d| d != u64::MAX).max().unwrap_or(0);
        let span = farthest(false).saturating_add(farthest(true));
        let scale = u32::try_from(span.div_ceil(u16::MAX as u64 - 1).max(1))
            .map_err(|_| "Landmark distances exceed the scale range")?;
        let mut nearest = vec![u64::MAX; self.first[0].len() - 1];
        let mut tables = Vec::new();
        for _ in 0..LANDMARKS {
            let distances = self.distances(costs, next, scale as u64, false);
            for &node in &component {
                nearest[node as usize] = nearest[node as usize].min(distances[node as usize]);
            }
            next = *component.iter().max_by_key(|&&node| nearest[node as usize]).unwrap();
            tables.push(landmarks::write(distances.into_iter().map(|d| d.min(u16::MAX as u64) as u32), write)?);
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
    fn compressed_bounds_stay_feasible_with_sparse_junction_ids_and_a_scale_for_the_region() {
        let endpoints = [(10, 20), (20, 30), (30, 10), (30, 40), (40, 30), (2_000_000_000, 2_000_000_001)];
        let graph = Junctions::new(endpoints.into_iter()).unwrap();
        let mut objects = BTreeMap::new();
        let mut write = |bytes: &[u8]| {
            let key = planner_router::package::digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        };
        let mut index = graph.index(&mut write).unwrap();
        let metrics = [vec![9, 15, 8, u32::MAX as u64 + 8, u32::MAX as u64 + 17, 9], vec![u64::MAX, 15, 8, 9, 17, 9]];
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
        assert_eq!(read(&index.mapping), graph.to);
        // The far junction of the costly roads is a landmark, and the scale keeps it below the cap.
        assert!(index.profiles["0"].tables.iter().any(|table| read(table)[graph.to[3] as usize] == 0));
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
                    if cost != u64::MAX {
                        assert!(
                            distances[graph.from[road] as usize] as u64 * columns.scale as u64
                                <= cost + distances[graph.to[road] as usize] as u64 * columns.scale as u64
                        );
                    }
                }
            }
        }
        // Only the junction outside the component stays at the cap.
        assert!(saturated);
        for table in &index.profiles["0"].tables {
            let distances = read(table);
            assert!((0..4).all(|road| distances[graph.from[road] as usize] < u16::MAX as u32));
        }
    }
}
