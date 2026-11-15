//! The final order of the roads: one sort by (key, emit index), done bucket by bucket while the
//! roads are copied once into their final columns.

use crate::column::Column;
use crate::partition::{self, Partition};
use crate::terrain::Height;
use crate::{sort, Graph, RoadSource, Roads};
use planner_router::model::Road;

/// The order key of a road between the walk-1 junctions `from` and `to`: its level-1 cell, which
/// is that of its arrival junction, then its border level, highest first, then its arrival
/// junction. So the roads of a cell are one range, and its entering border roads of each level
/// come first.
pub fn partition_key(from: u32, to: u32, partition: &Partition) -> u64 {
    let rank = partition.rank.values();
    let (from, to) = (rank[from as usize], rank[to as usize]);
    let cells = &partition.cells;
    let cell = cells.cell(to);
    let first = cells.level1.values()[cell][0];
    (cell as u64) << 32 | u64::from(255 - cells.border(from, to)) << 24 | u64::from(to - first)
}

/// `key(from, to)` of each road of `ends`, in emit order.
pub fn keys(ends: &Column<(u32, u32)>, key: impl Fn(u32, u32) -> Result<u64, String>) -> Result<Column<u64>, String> {
    ends.values().iter().map(|&(from, to)| key(from, to)).collect()
}

/// The number of buckets, from 1 to 256: the roads of one bucket before and after the terrain take
/// about one sort run, so its random reads and writes stay in the page cache that a sort run
/// assumes. `points` counts the shape points, and with terrain also the resampled points.
pub fn bucket_count(roads: u64, points: u64) -> Result<usize, String> {
    let run = crate::work::sort_run_bytes()? as u64;
    Ok((60 * roads + 12 * points).div_ceil(run).clamp(1, 256) as usize)
}

/// The keys that cut the roads into `buckets` buckets of about equal counts, without repeats. The
/// bounds change only the speed of `Buckets`, never the order.
pub fn bounds(keys: &Column<u64>, buckets: usize) -> Vec<u64> {
    let step = keys.len().div_ceil(buckets.max(1)).max(1);
    let mut bounds: Vec<u64> =
        sort::sorted(keys.clone()).enumerate().filter(|(i, _)| i % step == 0 && *i > 0).map(|(_, key)| key).collect();
    bounds.dedup();
    bounds
}

/// The roads of a bucket, and (key, (emit index, position in the bucket)) of each.
type Bucket = (Roads, Column<(u64, (u32, u32))>);

/// The roads in buckets of their keys. The bucket of key `k` is the number of bounds at or below
/// `k`, so equal keys share a bucket.
pub struct Buckets {
    bounds: Vec<u64>,
    buckets: Vec<Bucket>,
    emitted: u32,
}

impl Buckets {
    pub fn new(bounds: Vec<u64>) -> Self {
        let buckets = (0..=bounds.len()).map(|_| Default::default()).collect();
        Self { bounds, buckets, emitted: 0 }
    }

    pub fn push(&mut self, road: Road, source: RoadSource, key: u64) -> Result<(), String> {
        let (roads, keys) = &mut self.buckets[self.bounds.partition_point(|&bound| bound <= key)];
        keys.push((key, (self.emitted, roads.len() as u32)));
        roads.push(road, source);
        self.emitted = self.emitted.checked_add(1).ok_or("Too many roads")?;
        Ok(())
    }

    /// The roads in the order of one sort by (key, emit index), and the emit index of each. Each
    /// bucket is sorted, copied and dropped in turn. With `terrain`, the roads get their heights in
    /// the copy.
    pub fn finish(self, mut terrain: Option<Height>) -> Result<(Roads, Column<u32>), String> {
        let (mut roads, mut emitted) = (Roads::default(), Column::default());
        for (bucket, keys) in self.buckets {
            let mut order = Column::default();
            for (_, (index, position)) in sort::sorted(keys) {
                emitted.push(index);
                order.push(position);
            }
            roads.append_permuted(&bucket, order.values(), terrain.as_mut().map(|height| &mut **height as Height))?;
        }
        Ok((roads, emitted))
    }
}

/// Puts the junctions and the roads of a graph that a test builds in memory into the order of the
/// bake, with cells of at most `sizes` junctions and the heights of `terrain`, and the turn rules
/// onto the new road IDs. The bake orders its roads in the import. Returns the old ID of each road.
pub fn order(graph: &mut Graph, sizes: &[u32], terrain: Option<Height>) -> Result<Column<u32>, String> {
    partition::check_sizes(sizes)?;
    let old = std::mem::take(&mut graph.roads);
    let ends = (0..old.len()).map(|road| (old.from(road), old.to(road))).collect();
    let partition = partition::partition(&graph.points, &ends, sizes)?;
    let keys = keys(&ends, |from, to| Ok(partition_key(from, to, &partition)))?;
    let points = old.shapes.values().len() as u64;
    let mut buckets = Buckets::new(bounds(&keys, bucket_count(old.len() as u64, points)?));
    let rank = partition.rank.values();
    for (id, &key) in keys.values().iter().enumerate() {
        let mut road = Road::default();
        old.get(id, &mut road);
        (road.from, road.to) = (rank[road.from as usize], rank[road.to as usize]);
        buckets.push(road, RoadSource { way_row: old.way_row(id), closures: old.closures(id) }, key)?;
    }
    drop(old);
    let (roads, order) = buckets.finish(terrain)?;
    graph.roads = roads;
    let mut inverse = Column::filled(order.len(), 0);
    let inverse = inverse.values_mut();
    for (new, &old) in order.values().iter().enumerate() {
        inverse[old as usize] = new as u32;
    }
    for turns in [graph.forbidden.values_mut(), graph.forbidden_foot.values_mut()] {
        for crate::Turn { from, to } in turns.iter_mut() {
            *from = *inverse.get(*from as usize).ok_or("Unknown restriction road")?;
            *to = *inverse.get(*to as usize).ok_or("Unknown restriction road")?;
        }
        turns.sort_unstable();
    }
    graph.permute_junctions(&partition.order)?;
    let mut cells = partition.cells;
    cells.finish(&graph.roads)?;
    graph.cells = cells;
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain;
    use planner_router::model::{Point, NO_ELEVATION};

    #[test]
    fn buckets_give_the_order_of_one_sort() {
        let point = |lat, lon| Point { lat, lon, elevation: NO_ELEVATION };
        let roads: Vec<Road> = (0..1000u32)
            .map(|i| {
                let lon = (i * 7919 % 1000) as i32 * 30;
                let shape = vec![point(0, lon), point(50, lon + 700), point(90, lon + 750)];
                Road {
                    from: i,
                    to: i + 1,
                    way: i64::from(i),
                    length_m: i,
                    structure: i % 3 == 0,
                    shape,
                    ..Road::default()
                }
            })
            .collect();
        let keys: Column<u64> = (0..1000u64).map(|i| i * 31 % 97).collect();
        let height = |p: Point| Ok(Some(f64::from(p.lon % 500) / 7.0 + f64::from(p.lat)));
        let mut sorted: Vec<_> = keys.values().iter().zip(0u32..).map(|(&key, i)| (key, i)).collect();
        sorted.sort_unstable();
        let emit_order: Vec<_> = sorted.iter().map(|&(_, i)| i).collect();
        assert_eq!(bounds(&keys, 5).len(), 4);
        for buckets in [1, 5] {
            for with_terrain in [false, true] {
                let mut bucketed = Buckets::new(bounds(&keys, buckets));
                for ((road, &key), i) in roads.iter().zip(keys.values()).zip(0u32..) {
                    bucketed.push(road.clone(), RoadSource { way_row: i, closures: 5000 + i }, key).unwrap();
                }
                let mut h = height;
                let (result, emitted) = bucketed.finish(with_terrain.then_some(&mut h as Height)).unwrap();
                assert_eq!(emitted.values(), emit_order);
                let sources: Vec<_> = (0..result.len()).map(|i| (result.way_row(i), result.closures(i))).collect();
                assert_eq!(sources, emit_order.iter().map(|&i| (i, 5000 + i)).collect::<Vec<_>>());
                let expected: Vec<Road> = emit_order
                    .iter()
                    .map(|&i| {
                        let mut road = roads[i as usize].clone();
                        if with_terrain {
                            let mut shape = vec![Point::default(); terrain::resampled_len(&road.shape)];
                            let mut h = height;
                            (road.ascent_m, road.descent_m) =
                                terrain::sample(&road.shape, road.structure, &mut h, &mut shape).unwrap();
                            road.shape = shape;
                        }
                        road
                    })
                    .collect();
                let json = |roads: Vec<Road>| serde_json::to_value(roads).unwrap();
                assert_eq!(json(result.iter().collect()), json(expected), "{buckets} buckets, terrain {with_terrain}");
            }
        }
    }
}
