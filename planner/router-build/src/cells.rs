//! Writes the route package of format 10: one zstd object for each part of each download cell and
//! of each upper cell, `package.json`, and the cell directory `directory.bin`.

use crate::base::Topology;
use crate::partition::Cells;
use crate::tables::Tables;
use crate::{Costs, Graph};
use planner_router::cells::{
    self as format, align, class_entry, encode_group_cell, encode_road, encode_set, width, Cell, GeometryCell, Header,
    Level1, Part, Reference, Upper, GEOMETRY_RECORD, LONG_ROAD_M, PART_HEADER, TOPOLOGY_RECORD, UPPER_RECORD,
};
use planner_router::model::{Point, Road};
use rayon::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub const PACKAGE: &str = "package.json";
pub const DIRECTORY: &str = "directory.bin";
/// The zstd level of the objects. The owner chose 19 unless it takes more than 4 times the time of
/// 12 for less than 5 % fewer bytes; on Switzerland it does, so the objects use 12.
pub const LEVEL: i32 = 12;

/// The texts of `package.json` that the bake does not count.
pub struct Info {
    pub region: String,
    pub bounds: [f64; 4],
    pub source_sha256: Vec<String>,
    pub attribution: String,
}

/// The roads layer, the costs layer and the settings of a package.
pub struct Input<'a> {
    pub graph: &'a Graph,
    pub topology: &'a Topology,
    pub costs: &'a Costs,
    /// The cost tables of the costs layer.
    pub tables: &'a Tables,
    /// `cell_sizes` of the roads layer.
    pub sizes: &'a [u32],
    /// The setting; the package takes the smaller of it and the levels of the partition.
    pub download_level: u8,
    pub info: Info,
    /// The zstd level of the objects.
    pub level: i32,
}

/// What `write` wrote: the bytes by part kind and by column, the objects and the long roads.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub roads: u64,
    pub turns: u64,
    pub download_level: u8,
    pub cells: Vec<u32>,
    /// The profile names of each group.
    pub groups: Vec<Vec<String>>,
    /// The installed bytes of each column, as `part.column`.
    pub columns: BTreeMap<String, u64>,
    /// The installed and the published bytes of each part kind.
    pub parts: BTreeMap<String, [u64; 2]>,
    /// The size of each object.
    pub objects: Vec<u64>,
    pub long_roads: u64,
    /// The longest long list of one download cell.
    pub largest_long_list: u64,
    /// The installed bytes of the cost tables of each level.
    pub tables: Vec<u64>,
    /// The installed bytes of the tables that are equal to the table of an earlier profile of the
    /// same group and cell.
    pub equal_tables: u64,
    /// The SHA-256 of `directory.bin`: the identity of the package.
    pub identity: String,
}

impl Summary {
    fn add(&mut self, other: Summary) {
        for (name, bytes) in other.columns {
            *self.columns.entry(name).or_default() += bytes;
        }
        for (kind, [installed, published]) in other.parts {
            let entry = self.parts.entry(kind).or_default();
            entry[0] += installed;
            entry[1] += published;
        }
        self.objects.extend(other.objects);
        self.tables.resize(self.tables.len().max(other.tables.len()), 0);
        self.tables.iter_mut().zip(other.tables).for_each(|(bytes, other)| *bytes += other);
        self.equal_tables += other.equal_tables;
        self.long_roads += other.long_roads;
        self.largest_long_list = self.largest_long_list.max(other.largest_long_list);
    }

    fn column(&mut self, part: &str, column: &str, bytes: usize) {
        *self.columns.entry(format!("{part}.{column}")).or_default() += bytes as u64;
    }

    pub fn to_json(&self) -> Value {
        let mut sizes = self.objects.clone();
        sizes.sort_unstable();
        let (smallest, median, largest) = match sizes.len() {
            0 => (0, 0, 0),
            n => (sizes[0], sizes[n / 2], sizes[n - 1]),
        };
        json!({
            "identity": self.identity, "roads": self.roads, "turns": self.turns,
            "download_level": self.download_level, "cells": self.cells, "groups": self.groups,
            "columns": self.columns, "parts": self.parts,
            "objects": {"count": sizes.len(), "bytes": sizes.iter().sum::<u64>(),
                        "smallest": smallest, "median": median, "largest": largest},
            "long_roads": self.long_roads, "largest_long_list": self.largest_long_list,
            "tables": self.tables, "equal_tables": self.equal_tables,
        })
    }
}

/// A box of microdegrees: west, south, east, north.
type Bounds = [i32; 4];

fn grow(bounds: &mut Option<Bounds>, point: Point) {
    let b = bounds.get_or_insert([point.lon, point.lat, point.lon, point.lat]);
    *b = [b[0].min(point.lon), b[1].min(point.lat), b[2].max(point.lon), b[3].max(point.lat)];
}

fn union(bounds: &mut Option<Bounds>, other: Bounds) {
    grow(bounds, Point { lon: other[0], lat: other[1], elevation: 0.0 });
    grow(bounds, Point { lon: other[2], lat: other[3], elevation: 0.0 });
}

fn shape_bounds(shape: &[Point]) -> Bounds {
    let mut bounds = None;
    shape.iter().for_each(|&point| grow(&mut bounds, point));
    bounds.unwrap_or_default()
}

fn put(out: &mut Vec<u8>, value: u32, width: u8) {
    out.extend(&value.to_le_bytes()[..usize::from(width)]);
}

fn pad(out: &mut Vec<u8>, to: usize) {
    out.resize(align(out.len(), to), 0);
}

fn offset(len: usize) -> Result<u32, String> {
    u32::try_from(len).map_err(|_| "a part reaches 4 GiB".to_string())
}

/// The profiles in name order, and the profiles of each group: two profiles are one group when
/// their turn costs are equal. The groups are numbered by their first profile in name order.
fn groups(costs: &Costs) -> Result<(Vec<usize>, Vec<Vec<usize>>), String> {
    let mut order: Vec<usize> = (0..costs.profiles.len()).collect();
    order.sort_by(|&a, &b| costs.profiles[a].name.cmp(&costs.profiles[b].name));
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &profile in &order {
        match groups.iter_mut().find(|group| costs.turns(group[0]) == costs.turns(profile)) {
            Some(group) => group.push(profile),
            None => groups.push(vec![profile]),
        }
    }
    if groups.len() > format::MAX_GROUPS {
        return Err(format!("the profiles make {} groups; format 10 allows {}", groups.len(), format::MAX_GROUPS));
    }
    Ok((order, groups))
}

/// The partition as the writer reads it.
struct Tree<'a> {
    cells: &'a Cells,
    level1: &'a [[u32; 2]],
    ids: &'a [u32],
    /// The first road of each download cell.
    download_roads: Vec<u32>,
    download_ids: Vec<u32>,
    download_level: usize,
}

impl Tree<'_> {
    /// The reference of the road with run-time ID `road`.
    fn reference(&self, road: u32) -> Reference {
        let cell = self.download_roads.partition_point(|&first| first <= road) - 1;
        Reference { cell: self.download_ids[cell], index: road - self.download_roads[cell] }
    }

    /// The entering and the leaving list of cell `cell` of `level`, from 2.
    fn lists(&self, level: usize, cell: usize) -> (Vec<Reference>, Vec<Reference>) {
        let entering = self.cells.entering(level, cell).map(|road| self.reference(road));
        let leaving = self.cells.leaving_list(level, cell).iter().map(|&road| self.reference(road));
        (entering.collect(), leaving.collect())
    }
}

/// Appends the slots and then the tables of group `group` to the group part `out`: for each profile
/// of the group, the table of each cell of `cells` (level, cell), each at a multiple of 4. Returns
/// the offset of the slots.
fn slots(
    tables: &Tables,
    group: &[usize],
    cells: &[(usize, usize)],
    out: &mut Vec<u8>,
    stats: &mut Summary,
) -> Result<u32, String> {
    pad(out, 4);
    let slots_at = out.len();
    out.resize(slots_at + 8 * group.len() * cells.len(), 0);
    stats.column("group", "slots", out.len() - slots_at);
    let start = out.len();
    for (k, &p) in group.iter().enumerate() {
        for (i, &(level, cell)) in cells.iter().enumerate() {
            let table = tables.table(level, cell, p);
            if group[..k].iter().any(|&earlier| tables.table(level, cell, earlier) == table) {
                stats.equal_tables += table.len() as u64;
            }
            stats.tables.resize(stats.tables.len().max(level), 0);
            stats.tables[level - 1] += table.len() as u64;
            pad(out, 4);
            let slot = slots_at + 8 * (k * cells.len() + i);
            let at = offset(out.len())?;
            out[slot..slot + 4].copy_from_slice(&at.to_le_bytes());
            out[slot + 4..slot + 8].copy_from_slice(&offset(table.len())?.to_le_bytes());
            out.extend(table);
        }
    }
    stats.column("group", "tables", out.len() - start);
    offset(slots_at)
}

fn references(list: &[Reference], out: &mut Vec<u8>) {
    for reference in list {
        out.extend(reference.cell.to_le_bytes());
        out.extend(reference.index.to_le_bytes());
    }
}

/// Appends the upper records of `uppers` (level, cell) and their lists to the topology part `out`.
fn uppers(tree: &Tree, uppers: &[(usize, usize)], out: &mut Vec<u8>, stats: &mut Summary) -> Result<u32, String> {
    pad(out, 4);
    let upper_at = out.len();
    let lists: Vec<_> = uppers.iter().map(|&(level, cell)| tree.lists(level, cell)).collect();
    let mut at = upper_at + UPPER_RECORD * uppers.len();
    for (&(level, cell), (entering, leaving)) in uppers.iter().zip(&lists) {
        let entering_at = at;
        let leaving_at = entering_at + format::REFERENCE * entering.len();
        at = leaving_at + format::REFERENCE * leaving.len();
        let upper = Upper {
            level: level as u8,
            id: tree.cells.id(level, cell),
            entering_at: offset(entering_at)?,
            entering: entering.len() as u32,
            leaving_at: offset(leaving_at)?,
            leaving: leaving.len() as u32,
        };
        upper.encode(out);
    }
    for (entering, leaving) in &lists {
        references(entering, out);
        references(leaving, out);
    }
    stats.column("topology", "upper", out.len() - upper_at);
    offset(upper_at)
}

/// The parts of one download cell, before compression.
struct Parts {
    topology: Vec<u8>,
    geometry: Vec<u8>,
    groups: Vec<Vec<u8>>,
    bounds: Vec<Bounds>,
    long_bounds: Option<Bounds>,
    stats: Summary,
}

/// Builds the parts of download cell `d`.
fn download(input: &Input, tree: &Tree, groups: &[Vec<usize>], d: usize) -> Result<Parts, String> {
    let (roads, costs) = (&input.graph.roads, input.costs);
    let (first, head) = (input.topology.first.values(), input.topology.head.values());
    let level = tree.download_level;
    let l1s = tree.cells.level1_cells(level, d);
    let first_road = tree.level1[l1s.start][1] as usize;
    let mut stats = Summary::default();

    // The topology, and the order of the turns of each level-1 cell as topology arcs.
    let mut topology = vec![0; PART_HEADER + TOPOLOGY_RECORD * l1s.len()];
    let (mut records, mut arcs) = (Vec::new(), Vec::new());
    for l1 in l1s.clone() {
        let range = tree.cells.roads(l1);
        let count = range.len() as u32;
        let leaving = tree.cells.leaving_list(1, l1);
        let mut order = Vec::with_capacity((first[range.end] - first[range.start]) as usize);
        let mut firsts = vec![0u32];
        for road in range.clone() {
            let start = order.len();
            for arc in first[road]..first[road + 1] {
                order.push((tree.cells.target(l1, head[arc as usize])?, arc));
            }
            order[start..].sort_unstable();
            firsts.push(order.len() as u32);
        }
        let entering: Vec<u32> = tree.cells.entering(1, l1).collect();
        let too_many = |n: usize| u16::try_from(n).map_err(|_| format!("level-1 cell {l1} has {n} border roads"));
        let cell = Level1 {
            id: tree.ids[l1],
            first_road: (range.start - first_road) as u32,
            roads: count,
            turns: order.len() as u32,
            block: 0,
            entering: too_many(entering.len())?,
            leaving: too_many(leaving.len())?,
            first_width: width(Some(order.len() as u32)),
            head_width: width(order.iter().map(|&(local, _)| local).max()),
        };
        pad(&mut topology, 4);
        let block = topology.len();
        firsts.iter().for_each(|&value| put(&mut topology, value, cell.first_width));
        stats.column("topology", "first", topology.len() - block);
        pad(&mut topology, cell.head_width.into());
        let at = topology.len();
        order.iter().for_each(|&(local, _)| put(&mut topology, local, cell.head_width));
        stats.column("topology", "head", topology.len() - at);
        pad(&mut topology, 4);
        let at = topology.len();
        references(&leaving.iter().map(|&road| tree.reference(road)).collect::<Vec<_>>(), &mut topology);
        stats.column("topology", "leaving", topology.len() - at);
        let at = topology.len();
        for road in entering {
            let departure = tree.ids[tree.cells.cell(roads.from(road as usize))];
            topology.extend(departure.to_le_bytes());
        }
        stats.column("topology", "departure", topology.len() - at);
        records.push(Level1 { block: offset(block)?, ..cell });
        arcs.push(order.into_iter().map(|(_, arc)| arc).collect::<Vec<_>>());
    }
    let inside: Vec<(usize, usize)> =
        (2..=level).flat_map(|upper| tree.cells.within(upper, l1s.clone()).map(move |cell| (upper, cell))).collect();
    let upper_at = uppers(tree, &inside, &mut topology, &mut stats)?;
    let mut header = Vec::new();
    let turns: usize = records.iter().map(|cell| cell.turns as usize).sum();
    let road_count = tree.level1[l1s.end][1] as usize - first_road;
    format::part_header(
        [l1s.len() as u32, road_count as u32, turns as u32, inside.len() as u32, upper_at],
        &mut header,
    );
    records.iter().for_each(|cell| cell.encode(&mut header));
    topology[..header.len()].copy_from_slice(&header);
    stats.column("topology", "records", header.len());

    // The groups.
    let mut parts = Vec::new();
    for group in groups {
        let profiles = group.len();
        let class = |road: usize, key: &mut Vec<u8>| {
            key.clear();
            group.iter().for_each(|&p| key.extend(class_entry(costs.basis(p, road))));
        };
        let mut key = Vec::new();
        let mut distinct = BTreeSet::new();
        for road in first_road..first_road + road_count {
            class(road, &mut key);
            if !distinct.contains(&key) {
                distinct.insert(key.clone());
            }
        }
        let index: HashMap<Vec<u8>, u32> = distinct.iter().cloned().zip(0..).collect();
        let mut part = vec![0; PART_HEADER + format::group_record(profiles) * l1s.len()];
        let mut cells = Vec::new();
        for (l1, arcs) in l1s.clone().zip(&arcs) {
            let range = tree.cells.roads(l1);
            let finite = |value: u32| (value != u32::MAX).then_some(value);
            let turns = costs.turns(group[0]);
            let classes: Vec<u32> = range
                .clone()
                .map(|road| {
                    class(road, &mut key);
                    index[&key]
                })
                .collect();
            let turn_width = width(arcs.iter().filter_map(|&arc| finite(turns[arc as usize])).max());
            let class_width = width(classes.iter().copied().max());
            let cost_widths: Vec<u8> = group
                .iter()
                .map(|&p| width(range.clone().filter_map(|road| finite(costs.total(p, road))).max()))
                .collect();
            pad(&mut part, 4);
            let block = part.len();
            let all_ones = |w| format::all_ones(w);
            for &arc in arcs {
                put(&mut part, finite(turns[arc as usize]).unwrap_or(all_ones(turn_width)), turn_width);
            }
            stats.column("group", "turns", part.len() - block);
            pad(&mut part, class_width.into());
            let at = part.len();
            classes.iter().for_each(|&class| put(&mut part, class, class_width));
            stats.column("group", "classes", part.len() - at);
            for (&p, &w) in group.iter().zip(&cost_widths) {
                pad(&mut part, w.into());
                let at = part.len();
                range.clone().for_each(|road| put(&mut part, finite(costs.total(p, road)).unwrap_or(all_ones(w)), w));
                stats.column("group", "costs", part.len() - at);
            }
            for &p in group {
                let at = part.len();
                part.resize(at + range.len().div_ceil(8), 0);
                for (i, road) in range.clone().enumerate() {
                    part[at + i / 8] |= u8::from(costs.snappable(p, road)) << (i % 8);
                }
                stats.column("group", "allowed", part.len() - at);
            }
            cells.push((offset(block)?, turn_width, class_width, cost_widths));
        }
        pad(&mut part, 8);
        let classes_at = part.len();
        distinct.iter().for_each(|key| part.extend(key));
        stats.column("group", "class table", part.len() - classes_at);
        let tabled: Vec<(usize, usize)> = l1s.clone().map(|l1| (1, l1)).chain(inside.iter().copied()).collect();
        let slots_at = slots(input.tables, group, &tabled, &mut part, &mut stats)?;
        let mut header = Vec::new();
        let fields =
            [l1s.len() as u32, inside.len() as u32, profiles as u32, distinct.len() as u32, offset(classes_at)?];
        fields.iter().chain([&slots_at]).for_each(|value| header.extend(value.to_le_bytes()));
        cells
            .iter()
            .for_each(|(block, turns, classes, costs)| encode_group_cell(*block, *turns, *classes, costs, &mut header));
        part[..header.len()].copy_from_slice(&header);
        stats.column("group", "records", header.len());
        parts.push(part);
    }

    // The geometry.
    let mut geometry = vec![0; PART_HEADER + GEOMETRY_RECORD * l1s.len()];
    let (mut cells, mut bounds, mut long, mut long_bounds) = (Vec::new(), Vec::new(), Vec::new(), None);
    let (mut sets, mut set_ids) = (Vec::<u8>::new(), HashMap::new());
    let mut set_offsets = vec![0u32];
    let (mut road, mut records) = (Road::default(), Vec::new());
    for l1 in l1s.clone() {
        let range = tree.cells.roads(l1);
        let mut cell_bounds = None;
        for id in range.clone() {
            let shape = roads.shape(id);
            if roads.attributes(id).length_m > LONG_ROAD_M {
                let shape = shape_bounds(shape);
                long.push(((id - first_road) as u32, shape));
                union(&mut long_bounds, shape);
            } else {
                shape.iter().for_each(|&point| grow(&mut cell_bounds, point));
            }
        }
        let cell_bounds = cell_bounds
            .or_else(|| {
                range.clone().next().and_then(|id| roads.shape(id).last()).map(|p| [p.lon, p.lat, p.lon, p.lat])
            })
            .unwrap_or_else(|| {
                let p = input.graph.points.values()[tree.level1[l1][0] as usize];
                [p.lon, p.lat, p.lon, p.lat]
            });
        let mut offsets = vec![0u32];
        records.clear();
        for id in range.clone() {
            roads.get(id, &mut road);
            let set = roads.closures(id);
            let closure = if set == u32::MAX {
                0
            } else {
                let next = set_ids.len() as u32;
                let local = *set_ids.entry(set).or_insert(next);
                if local == next {
                    encode_set(input.graph.sets.get(set), &mut sets)?;
                    set_offsets.push(offset(sets.len())?);
                }
                local + 1
            };
            encode_road(&road, closure, cell_bounds, &mut records)?;
            offsets.push(offset(records.len())?);
        }
        let offset_width = GeometryCell::offset_width(records.len());
        pad(&mut geometry, 4);
        let block = geometry.len();
        offsets.iter().for_each(|&value| put(&mut geometry, value, offset_width));
        stats.column("geometry", "offsets", geometry.len() - block);
        geometry.extend(&records);
        stats.column("geometry", "roads", records.len());
        cells.push(GeometryCell {
            bounds: cell_bounds,
            roads: range.len() as u32,
            block: offset(block)?,
            bytes: offset(records.len())?,
            offset_width,
        });
        bounds.push(cell_bounds);
    }
    pad(&mut geometry, 4);
    let long_at = geometry.len();
    for (index, shape) in &long {
        geometry.extend(index.to_le_bytes());
        shape.iter().for_each(|value| geometry.extend(value.to_le_bytes()));
    }
    stats.column("geometry", "long", geometry.len() - long_at);
    let sets_at = geometry.len();
    set_offsets.iter().for_each(|value| geometry.extend(value.to_le_bytes()));
    geometry.extend(&sets);
    stats.column("geometry", "sets", geometry.len() - sets_at);
    let mut header = Vec::new();
    let fields = [cells.len() as u32, long.len() as u32, offset(long_at)?, set_ids.len() as u32, offset(sets_at)?];
    format::part_header(fields, &mut header);
    cells.iter().for_each(|cell| cell.encode(&mut header));
    geometry[..header.len()].copy_from_slice(&header);
    stats.column("geometry", "records", header.len());
    stats.long_roads = long.len() as u64;
    stats.largest_long_list = long.len() as u64;
    Ok(Parts { topology, geometry, groups: parts, bounds, long_bounds, stats })
}

/// A zstd compressor of the objects: one frame of one thread, with its content size, without a
/// checksum, and a window of at most 8 MiB.
fn compressor(level: i32) -> Result<zstd::bulk::Compressor<'static>, String> {
    use zstd::zstd_safe::CParameter;
    let mut compressor = zstd::bulk::Compressor::new(level).map_err(|e| e.to_string())?;
    for parameter in [CParameter::WindowLog(23), CParameter::ChecksumFlag(false), CParameter::ContentSizeFlag(true)] {
        compressor.set_parameter(parameter).map_err(|e| e.to_string())?;
    }
    Ok(compressor)
}

/// Compresses `part` into an object named by its SHA-256 in `output`.
fn publish(
    part: &[u8],
    kind: &str,
    compressor: &mut zstd::bulk::Compressor,
    output: &Path,
    stats: &mut Summary,
) -> Result<Part, String> {
    let bytes = offset(part.len())?;
    let object = compressor.compress(part).map_err(|e| e.to_string())?;
    let name: [u8; 32] = Sha256::digest(&object).into();
    let hex: String = name.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = output.join(&hex);
    let temporary = output.join(format!(".{hex}.{}", rayon::current_thread_index().unwrap_or(0)));
    std::fs::write(&temporary, &object).map_err(|e| format!("{}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entry = stats.parts.entry(kind.to_string()).or_default();
    entry[0] += u64::from(bytes);
    entry[1] += object.len() as u64;
    stats.objects.push(object.len() as u64);
    Ok(Part { object: name, object_bytes: offset(object.len())?, sha256: Sha256::digest(part).into(), bytes })
}

/// A download level of the setting: 1 to 8.
pub fn download_level(level: u64) -> Result<u8, String> {
    u8::try_from(level)
        .ok()
        .filter(|level| (1..=format::MAX_LEVELS as u8).contains(level))
        .ok_or(format!("`download_level` is {level}; it needs 1 to {}", format::MAX_LEVELS))
}

/// Writes the objects, `package.json` and `directory.bin` of the package of `input` into the
/// empty folder `output`, on the `rayon` pool. The bytes do not depend on the thread count.
pub fn write(input: &Input, output: &Path) -> Result<Summary, String> {
    let (graph, cells) = (input.graph, &input.graph.cells);
    let levels = cells.levels();
    download_level(input.download_level.into())?;
    if input.sizes.len() < levels {
        return Err(format!("`cell_sizes` has {} values; the partition has {levels} levels", input.sizes.len()));
    }
    let download_level = usize::from(input.download_level).min(levels);
    let (order, groups) = groups(input.costs)?;
    let level1 = cells.level1.values();
    let counts = cells.counts.values();
    let downloads = counts[download_level - 1] as usize;
    let tree = Tree {
        cells,
        level1,
        ids: cells.ids.values(),
        download_roads: (0..downloads)
            .map(|d| cells.roads(cells.level1_cells(download_level, d).start).start as u32)
            .collect(),
        download_ids: (0..downloads).map(|d| cells.id(download_level, d)).collect(),
        download_level,
    };
    if !tree.download_ids.is_sorted_by(|a, b| a < b) || !tree.download_roads.is_sorted() {
        return Err("the download cell IDs do not ascend with their first road".into());
    }
    std::fs::create_dir_all(output).map_err(|e| format!("{}: {e}", output.display()))?;
    let roads = u32::try_from(graph.roads.len()).map_err(|_| "Too many roads")?;
    let turns = u32::try_from(input.topology.head.len()).map_err(|_| "Too many turns")?;

    let built = (0..downloads)
        .into_par_iter()
        .with_max_len(1)
        .map_init(
            || compressor(input.level),
            |compressor, d| {
                let compressor = compressor.as_mut().map_err(|e| e.clone())?;
                let mut parts = download(input, &tree, &groups, d)?;
                let mut stats = std::mem::take(&mut parts.stats);
                let mut entries = vec![
                    publish(&parts.topology, "topology", compressor, output, &mut stats)?,
                    publish(&parts.geometry, "geometry", compressor, output, &mut stats)?,
                ];
                for part in &parts.groups {
                    entries.push(publish(part, "group", compressor, output, &mut stats)?);
                }
                let range = tree.cells.level1_cells(download_level, d);
                let mut bounds = None;
                parts.bounds.iter().for_each(|&b| union(&mut bounds, b));
                let cell = Cell {
                    level: download_level as u8,
                    id: tree.download_ids[d],
                    first_road: level1[range.start][1],
                    roads: level1[range.end][1] - level1[range.start][1],
                    bounds: bounds.unwrap_or_default(),
                    long_bounds: parts.long_bounds.unwrap_or_default(),
                    parts: entries,
                };
                Ok((cell, parts.bounds, stats))
            },
        )
        .collect();
    let built = crate::cost::in_order(built)?;
    let level1_bounds: Vec<Bounds> = built.iter().flat_map(|(_, bounds, _)| bounds.iter().copied()).collect();
    let mut summary = Summary::default();
    let mut records = Vec::new();
    for (cell, _, stats) in built {
        summary.add(stats);
        records.push(cell);
    }
    let upper: Vec<(usize, usize)> = (download_level + 1..=levels)
        .flat_map(|level| (0..counts[level - 1] as usize).map(move |c| (level, c)))
        .collect();
    let built = upper
        .par_iter()
        .with_max_len(1)
        .map_init(
            || compressor(input.level),
            |compressor, &(level, c)| {
                let compressor = compressor.as_mut().map_err(|e| e.clone())?;
                let mut stats = Summary::default();
                let mut part = vec![0; PART_HEADER];
                let upper_at = uppers(&tree, &[(level, c)], &mut part, &mut stats)?;
                let mut header = Vec::new();
                format::part_header([0, 0, 0, 1, upper_at], &mut header);
                part[..PART_HEADER].copy_from_slice(&header);
                let mut parts = vec![publish(&part, "topology", compressor, output, &mut stats)?, Part::default()];
                for group in &groups {
                    let mut part = vec![0; PART_HEADER];
                    let slots_at = slots(input.tables, group, &[(level, c)], &mut part, &mut stats)?;
                    let fields = [0, 1, group.len() as u32, 0, PART_HEADER as u32, slots_at];
                    part[..PART_HEADER].copy_from_slice(&fields.map(u32::to_le_bytes).concat());
                    parts.push(publish(&part, "group", compressor, output, &mut stats)?);
                }
                let range = tree.cells.level1_cells(level, c);
                let mut bounds = None;
                level1_bounds[range.clone()].iter().for_each(|&b| union(&mut bounds, b));
                let cell = Cell {
                    level: level as u8,
                    id: cells.id(level, c),
                    first_road: level1[range.start][1],
                    roads: level1[range.end][1] - level1[range.start][1],
                    bounds: bounds.unwrap_or_default(),
                    long_bounds: [0; 4],
                    parts,
                };
                Ok((cell, stats))
            },
        )
        .collect();
    for (cell, stats) in crate::cost::in_order(built)? {
        summary.add(stats);
        records.push(cell);
    }

    let profiles: Vec<Value> = order
        .iter()
        .map(|&p| {
            let group = groups.iter().position(|group| group.contains(&p)).expect("each profile has a group");
            json!({"group": group, "profile": input.costs.profiles[p]})
        })
        .collect();
    let info = &input.info;
    let package = json!({
        "format": format::FORMAT, "region": info.region, "bounds": info.bounds,
        "source_sha256": info.source_sha256, "attribution": info.attribution, "warnings": graph.warnings,
        "profiles": profiles, "levels": input.sizes[..levels], "download_level": download_level,
        "counts": {"cells": counts, "long_roads": summary.long_roads, "roads": roads, "turns": turns},
    });
    let package = serde_json::to_vec(&crate::sort_keys(package)).map_err(|e| e.to_string())?;
    let mut level_bits = [0; format::MAX_LEVELS];
    level_bits[..levels].copy_from_slice(cells.widths.values());
    let header = Header {
        levels: levels as u8,
        download_level: download_level as u8,
        groups: groups.len() as u8,
        level_bits,
        cells: records.len() as u32,
        roads,
        info: Sha256::digest(&package).into(),
    };
    let mut directory = Vec::new();
    header.encode(&mut directory);
    records.iter().for_each(|cell| cell.encode(&mut directory));
    let write = |name: &str, bytes: &[u8]| {
        let path = output.join(name);
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    write(PACKAGE, &package)?;
    write(DIRECTORY, &directory)?;
    summary.identity = format!("{:x}", Sha256::digest(&directory));
    summary.roads = roads.into();
    summary.turns = turns.into();
    summary.download_level = download_level as u8;
    summary.cells = counts.to_vec();
    summary.groups =
        groups.iter().map(|group| group.iter().map(|&p| input.costs.profiles[p].name.clone()).collect()).collect();
    *summary.columns.entry("directory.records".into()).or_default() += directory.len() as u64;
    Ok(summary)
}
