//! Format 10 of the route package: read views over the bytes of the cell directory and of the parts
//! of a cell, the block layouts that the writer shares, and the geometry record of a road.
//!
//! Each read checks its offsets against the slice and gives `InvalidData` outside it, so a view
//! over damaged bytes never panics.

use crate::closures::{Closure, Kind, Set};
use crate::cost::CostBasis;
use crate::model::{Point, Road, Surface};
use crate::{Error, Result};

pub const FORMAT: u32 = 10;
pub const MAGIC: [u8; 8] = *b"OBCROUTE";
/// The bytes of the directory header.
pub const HEADER: usize = 64;
pub const MAX_LEVELS: usize = 8;
pub const MAX_GROUPS: usize = 32;
/// The bytes of a part entry of a cell record.
pub const PART: usize = 72;
/// The part number of the topology and of the geometry; profile group `g` is part `2 + g`.
pub const TOPOLOGY: usize = 0;
pub const GEOMETRY: usize = 1;
/// A road whose `length_m` is above this is a long road.
pub const LONG_ROAD_M: u32 = 10_000;
/// The bytes of a part header, of a level-1 record of the topology and of the geometry, of an
/// upper record, of a road reference, of a long-list record and of a class entry.
pub const PART_HEADER: usize = 24;
pub const TOPOLOGY_RECORD: usize = 28;
pub const GEOMETRY_RECORD: usize = 32;
pub const UPPER_RECORD: usize = 24;
pub const REFERENCE: usize = 8;
pub const LONG_RECORD: usize = 20;
pub const CLASS_ENTRY: usize = 24;
const KINDS: [Kind; 9] = [
    Kind::Permit,
    Kind::Private,
    Kind::Farm,
    Kind::Sidepath,
    Kind::Discouraged,
    Kind::Limited,
    Kind::Seasonal,
    Kind::Conditional,
    Kind::Unclear,
];

fn invalid(what: &str) -> Error {
    Error::InvalidData(format!("Invalid route cell data: {what}"))
}

/// `at` rounded up to a multiple of `to`.
pub fn align(at: usize, to: usize) -> usize {
    at.div_ceil(to) * to
}

/// The narrowest width whose all-ones value is above `max`, the largest finite value of a column;
/// width 1 for a column with no finite value.
pub fn width(max: Option<u32>) -> u8 {
    match max {
        Some(max) if max >= 0xFFFF => 4,
        Some(max) if max >= 0xFF => 2,
        _ => 1,
    }
}

/// The all-ones value of `width`.
pub fn all_ones(width: u8) -> u32 {
    u32::MAX >> (32 - 8 * u32::from(width))
}

/// The unsigned integer of `width` bytes at `at`.
pub fn int(bytes: &[u8], at: usize, width: u8) -> Result<u32> {
    let end = at.checked_add(usize::from(width)).ok_or_else(|| invalid("offset"))?;
    let field = bytes.get(at..end).ok_or_else(|| invalid("an offset outside the part"))?;
    Ok(field.iter().rev().fold(0, |value, &byte| value << 8 | u32::from(byte)))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    int(bytes, at, 4)
}

fn i32_at(bytes: &[u8], at: usize) -> Result<i32> {
    Ok(u32_at(bytes, at)? as i32)
}

fn bounds_at(bytes: &[u8], at: usize) -> Result<[i32; 4]> {
    Ok([i32_at(bytes, at)?, i32_at(bytes, at + 4)?, i32_at(bytes, at + 8)?, i32_at(bytes, at + 12)?])
}

fn byte(bytes: &[u8], at: usize) -> Result<u8> {
    bytes.get(at).copied().ok_or_else(|| invalid("an offset outside the part"))
}

fn hash_at(bytes: &[u8], at: usize) -> Result<[u8; 32]> {
    bytes.get(at..at + 32).and_then(|hash| hash.try_into().ok()).ok_or_else(|| invalid("a short hash"))
}

/// Fails unless `count` items of `size` bytes from `at` lie in `bytes`.
fn within(bytes: &[u8], at: usize, count: usize, size: usize) -> Result<()> {
    let end = count.checked_mul(size).and_then(|n| n.checked_add(at));
    end.filter(|&end| end <= bytes.len()).map(|_| ()).ok_or_else(|| invalid("a section outside the part"))
}

/// One part entry of a cell record. An absent part is all zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// SHA-256 of the published object.
    pub object: [u8; 32],
    pub object_bytes: u32,
    /// SHA-256 of the part: the installed file.
    pub sha256: [u8; 32],
    pub bytes: u32,
}

impl Part {
    pub fn absent(&self) -> bool {
        *self == Part::default()
    }
}

/// One cell record of the directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub level: u8,
    /// The cell ID, with zero bits below its level.
    pub id: u32,
    /// The run-time ID of its first road, and its road count.
    pub first_road: u32,
    pub roads: u32,
    /// West, south, east, north in microdegrees: the union of the boxes of its level-1 cells.
    pub bounds: [i32; 4],
    /// The union of the boxes of the long roads of a download cell; all zero without one.
    pub long_bounds: [i32; 4],
    /// Topology, geometry, then each profile group.
    pub parts: Vec<Part>,
}

impl Cell {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend([self.level, 0, 0, 0]);
        for value in [self.id, self.first_road, self.roads] {
            out.extend(value.to_le_bytes());
        }
        for value in self.bounds.iter().chain(&self.long_bounds) {
            out.extend(value.to_le_bytes());
        }
        for part in &self.parts {
            out.extend(part.object);
            out.extend(part.object_bytes.to_le_bytes());
            out.extend(part.sha256);
            out.extend(part.bytes.to_le_bytes());
        }
    }
}

/// The bytes of a cell record of a directory with `groups` profile groups.
pub fn record_bytes(groups: usize) -> usize {
    48 + PART * (2 + groups)
}

/// The header fields of the directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub levels: u8,
    pub download_level: u8,
    pub groups: u8,
    /// The bit count of the ID field of each level, level 1 first.
    pub level_bits: [u8; MAX_LEVELS],
    pub cells: u32,
    pub roads: u32,
    /// SHA-256 of `package.json`.
    pub info: [u8; 32],
}

impl Header {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend(MAGIC);
        out.extend(FORMAT.to_le_bytes());
        out.extend([self.levels, self.download_level, self.groups, 0]);
        out.extend(self.level_bits);
        out.extend(self.cells.to_le_bytes());
        out.extend(self.roads.to_le_bytes());
        out.extend(self.info);
    }
}

/// The cell directory, `directory.bin`.
#[derive(Clone, Copy, Debug)]
pub struct Directory<'a> {
    bytes: &'a [u8],
    pub header: Header,
}

impl<'a> Directory<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < HEADER || bytes[..8] != MAGIC || u32_at(bytes, 8)? != FORMAT {
            return Err(invalid("not a directory of format 10"));
        }
        let level_bits: [u8; MAX_LEVELS] = bytes[16..24].try_into().unwrap();
        let header = Header {
            levels: bytes[12],
            download_level: bytes[13],
            groups: bytes[14],
            level_bits,
            cells: u32_at(bytes, 24)?,
            roads: u32_at(bytes, 28)?,
            info: hash_at(bytes, 32)?,
        };
        let bits: u32 = level_bits[..usize::from(header.levels).min(MAX_LEVELS)].iter().map(|&b| u32::from(b)).sum();
        if !(1..=MAX_LEVELS as u8).contains(&header.levels)
            || !(1..=header.levels).contains(&header.download_level)
            || usize::from(header.groups) > MAX_GROUPS
            || bits > 32
            || bytes.len() != HEADER + header.cells as usize * record_bytes(header.groups.into())
        {
            return Err(invalid("a bad directory header"));
        }
        Ok(Self { bytes, header })
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.header.cells as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The parts of each record: topology, geometry and the groups.
    pub fn parts(&self) -> usize {
        2 + usize::from(self.header.groups)
    }

    /// Record `index`.
    pub fn cell(&self, index: usize) -> Result<Cell> {
        if index >= self.len() {
            return Err(invalid("a cell outside the directory"));
        }
        let at = HEADER + index * record_bytes(self.header.groups.into());
        let parts = (0..self.parts())
            .map(|part| {
                let at = at + 48 + part * PART;
                Ok(Part {
                    object: hash_at(self.bytes, at)?,
                    object_bytes: u32_at(self.bytes, at + 32)?,
                    sha256: hash_at(self.bytes, at + 36)?,
                    bytes: u32_at(self.bytes, at + 68)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Cell {
            level: self.bytes[at],
            id: u32_at(self.bytes, at + 4)?,
            first_road: u32_at(self.bytes, at + 8)?,
            roads: u32_at(self.bytes, at + 12)?,
            bounds: bounds_at(self.bytes, at + 16)?,
            long_bounds: bounds_at(self.bytes, at + 32)?,
            parts,
        })
    }

    fn field(&self, index: usize, at: usize) -> Result<u32> {
        u32_at(self.bytes, HEADER + index * record_bytes(self.header.groups.into()) + at)
    }

    /// The number of download cells: the first records.
    pub fn downloads(&self) -> usize {
        let level = |index| self.bytes[HEADER + index * record_bytes(self.header.groups.into())];
        (0..self.len()).position(|index| level(index) != self.header.download_level).unwrap_or(self.len())
    }

    /// The record of the download cell that holds the road with run-time ID `road`.
    pub fn download_of_road(&self, road: u32) -> Result<Option<usize>> {
        let (mut low, mut high) = (0, self.downloads());
        while low < high {
            let at = low + (high - low) / 2;
            if self.field(at, 8)? <= road {
                low = at + 1;
            } else {
                high = at;
            }
        }
        let Some(index) = low.checked_sub(1) else { return Ok(None) };
        Ok((road - self.field(index, 8)? < self.field(index, 12)?).then_some(index))
    }

    /// The bits of a cell ID below level `level`.
    pub fn below(&self, level: u8) -> u32 {
        let bits: u32 =
            self.header.level_bits[..usize::from(level.saturating_sub(1))].iter().map(|&b| u32::from(b)).sum();
        if bits >= 32 {
            u32::MAX
        } else {
            (1 << bits) - 1
        }
    }

    /// The record of the cell of `level` that holds the cell with ID `id`.
    pub fn find(&self, level: u8, id: u32) -> Result<Option<usize>> {
        let id = id & !self.below(level);
        let (start, end) =
            if level == self.header.download_level { (0, self.downloads()) } else { (self.downloads(), self.len()) };
        let key = |index: usize| {
            Ok::<_, Error>((
                self.bytes[HEADER + index * record_bytes(self.header.groups.into())],
                self.field(index, 4)?,
            ))
        };
        let (mut low, mut high) = (start, end);
        while low < high {
            let at = low + (high - low) / 2;
            if key(at)? < (level, id) {
                low = at + 1;
            } else {
                high = at;
            }
        }
        Ok((low < end && key(low)? == (level, id)).then_some(low))
    }
}

/// A road named by its download cell ID and its index in that cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub cell: u32,
    pub index: u32,
}

/// The column offsets of a topology block from its start, and its size.
#[derive(Clone, Copy, Debug)]
pub struct TopologyBlock {
    pub head: usize,
    pub leaving: usize,
    pub departure: usize,
    pub end: usize,
}

impl TopologyBlock {
    pub fn new(cell: &Level1) -> Self {
        let (roads, turns) = (cell.roads as usize, cell.turns as usize);
        let head = align((roads + 1) * usize::from(cell.first_width), cell.head_width.into());
        let leaving = align(head + turns * usize::from(cell.head_width), 4);
        let departure = leaving + REFERENCE * usize::from(cell.leaving);
        Self { head, leaving, departure, end: departure + 4 * usize::from(cell.entering) }
    }
}

/// A level-1 record of the topology part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Level1 {
    pub id: u32,
    /// Its first road, as an index in the download cell.
    pub first_road: u32,
    pub roads: u32,
    pub turns: u32,
    pub block: u32,
    pub entering: u16,
    pub leaving: u16,
    pub first_width: u8,
    pub head_width: u8,
}

impl Level1 {
    pub fn encode(&self, out: &mut Vec<u8>) {
        for value in [self.id, self.first_road, self.roads, self.turns, self.block] {
            out.extend(value.to_le_bytes());
        }
        out.extend(self.entering.to_le_bytes());
        out.extend(self.leaving.to_le_bytes());
        out.extend([self.first_width, self.head_width, 0, 0]);
    }
}

/// An upper record of the topology part: the two road lists of one cell above level 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Upper {
    pub level: u8,
    pub id: u32,
    pub entering_at: u32,
    pub entering: u32,
    pub leaving_at: u32,
    pub leaving: u32,
}

impl Upper {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend([self.level, 0, 0, 0]);
        for value in [self.id, self.entering_at, self.entering, self.leaving_at, self.leaving] {
            out.extend(value.to_le_bytes());
        }
    }
}

/// Encodes a part header of five u32 fields and a zero.
pub fn part_header(fields: [u32; 5], out: &mut Vec<u8>) {
    for value in fields.into_iter().chain([0]) {
        out.extend(value.to_le_bytes());
    }
}

/// The topology part of a cell.
#[derive(Clone, Copy, Debug)]
pub struct Topology<'a> {
    bytes: &'a [u8],
    pub cells: u32,
    pub roads: u32,
    pub turns: u32,
    pub upper: u32,
    upper_at: u32,
}

impl<'a> Topology<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let field = |k: usize| u32_at(bytes, 4 * k);
        let view =
            Self { bytes, cells: field(0)?, roads: field(1)?, turns: field(2)?, upper: field(3)?, upper_at: field(4)? };
        within(bytes, PART_HEADER, view.cells as usize, TOPOLOGY_RECORD)?;
        within(bytes, view.upper_at as usize, view.upper as usize, UPPER_RECORD)?;
        Ok(view)
    }

    pub fn level1(&self, index: u32) -> Result<Level1> {
        if index >= self.cells {
            return Err(invalid("a level-1 cell outside the part"));
        }
        let at = PART_HEADER + index as usize * TOPOLOGY_RECORD;
        let field = |k: usize| u32_at(self.bytes, at + 4 * k);
        let cell = Level1 {
            id: field(0)?,
            first_road: field(1)?,
            roads: field(2)?,
            turns: field(3)?,
            block: field(4)?,
            entering: int(self.bytes, at + 20, 2)? as u16,
            leaving: int(self.bytes, at + 22, 2)? as u16,
            first_width: byte(self.bytes, at + 24)?,
            head_width: byte(self.bytes, at + 25)?,
        };
        if ![cell.first_width, cell.head_width].iter().all(|w| [1, 2, 4].contains(w)) {
            return Err(invalid("a bad column width"));
        }
        within(self.bytes, cell.block as usize, 1, TopologyBlock::new(&cell).end)?;
        Ok(cell)
    }

    /// The turns of road `road` of `cell`, counted from the first turn of the cell.
    pub fn turns(&self, cell: &Level1, road: u32) -> Result<std::ops::Range<u32>> {
        if road >= cell.roads {
            return Err(invalid("a road outside its cell"));
        }
        let at = cell.block as usize + road as usize * usize::from(cell.first_width);
        let width = usize::from(cell.first_width);
        let (start, end) = (int(self.bytes, at, cell.first_width)?, int(self.bytes, at + width, cell.first_width)?);
        if start > end || end > cell.turns {
            return Err(invalid("a bad turn range"));
        }
        Ok(start..end)
    }

    /// The target of turn `turn` of `cell`: a road of the cell below `roads`, else `roads` plus its
    /// index in the leaving list of the cell.
    pub fn head(&self, cell: &Level1, turn: u32) -> Result<u32> {
        if turn >= cell.turns {
            return Err(invalid("a turn outside its cell"));
        }
        let at = cell.block as usize + TopologyBlock::new(cell).head + turn as usize * usize::from(cell.head_width);
        let head = int(self.bytes, at, cell.head_width)?;
        if head >= cell.roads + u32::from(cell.leaving) {
            return Err(invalid("a turn target outside the cell"));
        }
        Ok(head)
    }

    /// Entry `k` of the leaving list of `cell`.
    pub fn leaving(&self, cell: &Level1, k: u32) -> Result<Reference> {
        if k >= u32::from(cell.leaving) {
            return Err(invalid("a leaving road outside the list"));
        }
        self.reference(cell.block as usize + TopologyBlock::new(cell).leaving, k)
    }

    /// The level-1 cell ID of the departure junction of entering road `k` of `cell`.
    pub fn departure(&self, cell: &Level1, k: u32) -> Result<u32> {
        if k >= u32::from(cell.entering) {
            return Err(invalid("an entering road outside the list"));
        }
        u32_at(self.bytes, cell.block as usize + TopologyBlock::new(cell).departure + 4 * k as usize)
    }

    pub fn upper(&self, index: u32) -> Result<Upper> {
        if index >= self.upper {
            return Err(invalid("an upper record outside the part"));
        }
        let at = self.upper_at as usize + index as usize * UPPER_RECORD;
        let field = |k: usize| u32_at(self.bytes, at + 4 + 4 * k);
        let upper = Upper {
            level: byte(self.bytes, at)?,
            id: field(0)?,
            entering_at: field(1)?,
            entering: field(2)?,
            leaving_at: field(3)?,
            leaving: field(4)?,
        };
        within(self.bytes, upper.entering_at as usize, upper.entering as usize, REFERENCE)?;
        within(self.bytes, upper.leaving_at as usize, upper.leaving as usize, REFERENCE)?;
        Ok(upper)
    }

    /// Entry `k` of the list of road references at `at`.
    pub fn reference(&self, at: usize, k: u32) -> Result<Reference> {
        let at = at + REFERENCE * k as usize;
        Ok(Reference { cell: u32_at(self.bytes, at)?, index: u32_at(self.bytes, at + 4)? })
    }
}

/// The column offsets of a profile group block from its start, and its size.
#[derive(Clone, Debug)]
pub struct GroupBlock {
    pub classes: usize,
    pub costs: Vec<usize>,
    pub allowed: Vec<usize>,
    pub end: usize,
}

impl GroupBlock {
    pub fn new(roads: u32, turns: u32, cell: &GroupCell) -> Self {
        let roads = roads as usize;
        let classes = align(turns as usize * usize::from(cell.turn_width), cell.class_width.into());
        let mut at = classes + roads * usize::from(cell.class_width);
        let costs = cell
            .cost_widths
            .iter()
            .map(|&width| {
                let start = align(at, width.into());
                at = start + roads * usize::from(width);
                start
            })
            .collect();
        let allowed = (0..cell.cost_widths.len()).map(|p| at + p * roads.div_ceil(8)).collect();
        Self { classes, costs, allowed, end: at + cell.cost_widths.len() * roads.div_ceil(8) }
    }
}

/// A level-1 record of a profile group part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupCell {
    pub block: u32,
    pub turn_width: u8,
    pub class_width: u8,
    pub cost_widths: Vec<u8>,
}

impl GroupCell {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend(self.block.to_le_bytes());
        out.extend([self.turn_width, self.class_width]);
        out.extend(&self.cost_widths);
        out.resize(start + group_record(self.cost_widths.len()), 0);
    }
}

/// The bytes of a level-1 record of a group part of `profiles` profiles.
pub fn group_record(profiles: usize) -> usize {
    align(6 + profiles, 4)
}

/// A profile group part of a cell.
#[derive(Clone, Copy, Debug)]
pub struct Group<'a> {
    bytes: &'a [u8],
    pub cells: u32,
    pub upper: u32,
    pub profiles: u32,
    pub classes: u32,
    classes_at: u32,
    slots_at: u32,
}

impl<'a> Group<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let field = |k: usize| u32_at(bytes, 4 * k);
        let view = Self {
            bytes,
            cells: field(0)?,
            upper: field(1)?,
            profiles: field(2)?,
            classes: field(3)?,
            classes_at: field(4)?,
            slots_at: field(5)?,
        };
        let profiles = view.profiles as usize;
        within(bytes, PART_HEADER, view.cells as usize, group_record(profiles))?;
        within(bytes, view.classes_at as usize, view.classes as usize, CLASS_ENTRY * profiles)?;
        within(bytes, view.slots_at as usize, profiles * (view.cells + view.upper) as usize, 8)?;
        Ok(view)
    }

    /// The record of level-1 cell `index`, whose topology record is `cell`.
    pub fn level1(&self, index: u32, cell: &Level1) -> Result<GroupCell> {
        if index >= self.cells {
            return Err(invalid("a level-1 cell outside the part"));
        }
        let at = PART_HEADER + index as usize * group_record(self.profiles as usize);
        let widths =
            (0..self.profiles as usize + 2).map(|k| byte(self.bytes, at + 4 + k)).collect::<Result<Vec<_>>>()?;
        if !widths.iter().all(|w| [1, 2, 4].contains(w)) {
            return Err(invalid("a bad column width"));
        }
        let group = GroupCell {
            block: u32_at(self.bytes, at)?,
            turn_width: widths[0],
            class_width: widths[1],
            cost_widths: widths[2..].to_vec(),
        };
        within(self.bytes, group.block as usize, 1, GroupBlock::new(cell.roads, cell.turns, &group).end)?;
        Ok(group)
    }

    /// The cost of turn `turn` of `cell`; `None` where the profiles of the group cannot use it.
    pub fn turn(&self, cell: &Level1, group: &GroupCell, turn: u32) -> Result<Option<u32>> {
        if turn >= cell.turns {
            return Err(invalid("a turn outside its cell"));
        }
        let raw =
            int(self.bytes, group.block as usize + turn as usize * usize::from(group.turn_width), group.turn_width)?;
        Ok((raw != all_ones(group.turn_width)).then_some(raw))
    }

    /// The cost class of road `road` of `cell`.
    pub fn class(&self, cell: &Level1, group: &GroupCell, road: u32) -> Result<u32> {
        let at =
            GroupBlock::new(cell.roads, cell.turns, group).classes + road as usize * usize::from(group.class_width);
        let class = int(self.bytes, group.block as usize + at, group.class_width)?;
        if road >= cell.roads || class >= self.classes {
            return Err(invalid("a bad cost class"));
        }
        Ok(class)
    }

    /// The total cost of road `road` of `cell` for profile `profile` of the group; `None` where the
    /// profile excludes it.
    pub fn cost(&self, cell: &Level1, group: &GroupCell, profile: u32, road: u32) -> Result<Option<u32>> {
        let width = *group.cost_widths.get(profile as usize).ok_or_else(|| invalid("a profile outside the group"))?;
        if road >= cell.roads {
            return Err(invalid("a road outside its cell"));
        }
        let at =
            GroupBlock::new(cell.roads, cell.turns, group).costs[profile as usize] + road as usize * usize::from(width);
        let raw = int(self.bytes, group.block as usize + at, width)?;
        Ok((raw != all_ones(width)).then_some(raw))
    }

    /// The snap bit of road `road` of `cell` for profile `profile`.
    pub fn allowed(&self, cell: &Level1, group: &GroupCell, profile: u32, road: u32) -> Result<bool> {
        if profile >= self.profiles || road >= cell.roads {
            return Err(invalid("a road or a profile outside the part"));
        }
        let at = GroupBlock::new(cell.roads, cell.turns, group).allowed[profile as usize] + road as usize / 8;
        Ok(byte(self.bytes, group.block as usize + at)? >> (road % 8) & 1 != 0)
    }

    /// The cost basis of class `class` for profile `profile`; `None` where the profile excludes
    /// the road.
    pub fn basis(&self, class: u32, profile: u32) -> Result<Option<CostBasis>> {
        if class >= self.classes || profile >= self.profiles {
            return Err(invalid("a class outside the table"));
        }
        let at = self.classes_at as usize + (class * self.profiles + profile) as usize * CLASS_ENTRY;
        let word = |k: usize| -> Result<u64> {
            Ok(u64::from(u32_at(self.bytes, at + 8 * k)?) | u64::from(u32_at(self.bytes, at + 8 * k + 4)?) << 32)
        };
        let basis =
            CostBasis { factor: f64::from_bits(word(0)?), turn: f64::from_bits(word(1)?), ferry: word(2)? != 0 };
        if basis.factor == 0.0 {
            return Ok(None);
        }
        if !basis.valid() || word(2)? > 1 {
            return Err(invalid("a bad cost basis"));
        }
        Ok(Some(basis))
    }

    /// Slot `index` of profile `profile`: the offset and the size of a cost table.
    pub fn slot(&self, profile: u32, index: u32) -> Result<(u32, u32)> {
        let per_profile = self.cells + self.upper;
        if profile >= self.profiles || index >= per_profile {
            return Err(invalid("a slot outside the part"));
        }
        let at = self.slots_at as usize + 8 * (profile * per_profile + index) as usize;
        Ok((u32_at(self.bytes, at)?, u32_at(self.bytes, at + 4)?))
    }
}

/// The class entry of one profile: 24 bytes, all zero where the profile excludes the road.
pub fn class_entry(basis: Option<CostBasis>) -> [u8; CLASS_ENTRY] {
    let mut entry = [0; CLASS_ENTRY];
    if let Some(basis) = basis {
        entry[..8].copy_from_slice(&basis.factor.to_bits().to_le_bytes());
        entry[8..16].copy_from_slice(&basis.turn.to_bits().to_le_bytes());
        entry[16..].copy_from_slice(&u64::from(basis.ferry).to_le_bytes());
    }
    entry
}

/// A level-1 record of the geometry part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryCell {
    /// West, south, east, north of the shapes of its roads that are not long.
    pub bounds: [i32; 4],
    pub roads: u32,
    pub block: u32,
    /// The size of the records of the block.
    pub bytes: u32,
    pub offset_width: u8,
}

impl GeometryCell {
    pub fn encode(&self, out: &mut Vec<u8>) {
        for value in self.bounds {
            out.extend(value.to_le_bytes());
        }
        for value in [self.roads, self.block, self.bytes] {
            out.extend(value.to_le_bytes());
        }
        out.extend([self.offset_width, 0, 0, 0]);
    }

    /// The offset width of a block with `bytes` of records.
    pub fn offset_width(bytes: usize) -> u8 {
        if bytes < 0xFFFF {
            2
        } else {
            4
        }
    }
}

/// The geometry part of a download cell.
#[derive(Clone, Copy, Debug)]
pub struct Geometry<'a> {
    bytes: &'a [u8],
    pub cells: u32,
    pub long: u32,
    long_at: u32,
    pub sets: u32,
    sets_at: u32,
}

impl<'a> Geometry<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let field = |k: usize| u32_at(bytes, 4 * k);
        let view =
            Self { bytes, cells: field(0)?, long: field(1)?, long_at: field(2)?, sets: field(3)?, sets_at: field(4)? };
        within(bytes, PART_HEADER, view.cells as usize, GEOMETRY_RECORD)?;
        within(bytes, view.long_at as usize, view.long as usize, LONG_RECORD)?;
        within(bytes, view.sets_at as usize, view.sets as usize + 1, 4)?;
        Ok(view)
    }

    pub fn level1(&self, index: u32) -> Result<GeometryCell> {
        if index >= self.cells {
            return Err(invalid("a level-1 cell outside the part"));
        }
        let at = PART_HEADER + index as usize * GEOMETRY_RECORD;
        let cell = GeometryCell {
            bounds: bounds_at(self.bytes, at)?,
            roads: u32_at(self.bytes, at + 16)?,
            block: u32_at(self.bytes, at + 20)?,
            bytes: u32_at(self.bytes, at + 24)?,
            offset_width: byte(self.bytes, at + 28)?,
        };
        if ![2, 4].contains(&cell.offset_width) {
            return Err(invalid("a bad offset width"));
        }
        let offsets = (cell.roads as usize + 1) * usize::from(cell.offset_width);
        within(self.bytes, cell.block as usize + offsets, 1, cell.bytes as usize)?;
        Ok(cell)
    }

    /// Fills `road` with road `index` of `cell`. Returns its closure field: 0, or 1 plus the index
    /// of its closure set.
    pub fn road(&self, cell: &GeometryCell, index: u32, road: &mut Road) -> Result<u32> {
        if index >= cell.roads {
            return Err(invalid("a road outside its cell"));
        }
        let width = cell.offset_width;
        let at = cell.block as usize + index as usize * usize::from(width);
        let (start, end) = (int(self.bytes, at, width)?, int(self.bytes, at + usize::from(width), width)?);
        let records = cell.block as usize + (cell.roads as usize + 1) * usize::from(width);
        if start > end || end > cell.bytes {
            return Err(invalid("a bad record offset"));
        }
        let closure = decode_road(&self.bytes[records + start as usize..records + end as usize], cell.bounds, road)?;
        if closure > self.sets {
            return Err(invalid("a closure set outside the table"));
        }
        Ok(closure)
    }

    /// Entry `k` of the long list: the road as an index in the download cell, and its box.
    pub fn long(&self, k: u32) -> Result<(u32, [i32; 4])> {
        if k >= self.long {
            return Err(invalid("a long road outside the list"));
        }
        let at = self.long_at as usize + k as usize * LONG_RECORD;
        Ok((u32_at(self.bytes, at)?, bounds_at(self.bytes, at + 4)?))
    }

    /// Closure set `k`.
    pub fn set(&self, k: u32) -> Result<Set> {
        if k >= self.sets {
            return Err(invalid("a closure set outside the table"));
        }
        let first = self.sets_at as usize + 4 * (self.sets as usize + 1);
        let start = u32_at(self.bytes, self.sets_at as usize + 4 * k as usize)? as usize;
        let end = u32_at(self.bytes, self.sets_at as usize + 4 * k as usize + 4)? as usize;
        let bytes =
            self.bytes.get(first + start..first + end).ok_or_else(|| invalid("a closure set outside the part"))?;
        decode_set(bytes)
    }
}

pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_signed(out: &mut Vec<u8>, value: i64) {
    put_varint(out, (value << 1 ^ value >> 63) as u64);
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = byte(bytes, *at)?;
        *at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(invalid("a varint above 64 bits"))
}

fn signed(bytes: &[u8], at: &mut usize) -> Result<i64> {
    let value = varint(bytes, at)?;
    Ok((value >> 1) as i64 ^ -((value & 1) as i64))
}

fn small(value: u64) -> Result<u32> {
    u32::try_from(value).map_err(|_| invalid("a value above 32 bits"))
}

/// Appends the geometry record of `road` to `out`. `closure` is 0, or 1 plus the index of its
/// closure set; `bounds` is the box of its level-1 cell.
pub fn encode_road(road: &Road, closure: u32, bounds: [i32; 4], out: &mut Vec<u8>) -> std::result::Result<(), String> {
    let way = u64::try_from(road.way).map_err(|_| format!("way {} has a negative ID", road.way))?;
    let mtb = match road.difficulty {
        255 => 15,
        scale @ 0..=6 => scale,
        scale => return Err(format!("way {}: MTB scale {scale} does not fit the record", road.way)),
    };
    let sac = match road.hiking_difficulty {
        None => 15,
        Some(255) => 14,
        Some(scale @ 0..=6) => scale,
        Some(scale) => return Err(format!("way {}: SAC scale {scale} does not fit the record", road.way)),
    };
    if road.access > 7 || road.shape.len() < 2 {
        return Err(format!("way {}: bad access or fewer than two points", road.way));
    }
    put_varint(out, way);
    out.push(road.surface as u8 | road.access << 3);
    out.push(mtb | sac << 4);
    put_varint(out, road.length_m.into());
    put_varint(out, closure.into());
    put_varint(out, road.shape.len() as u64);
    let (mut previous, mut step) = ([i64::from(bounds[1]), i64::from(bounds[0])], [0i64; 2]);
    for (k, point) in road.shape.iter().enumerate() {
        let here = [i64::from(point.lat), i64::from(point.lon)];
        for axis in 0..2 {
            let delta = here[axis] - previous[axis];
            put_signed(out, if k < 2 { delta } else { delta - step[axis] });
            step[axis] = delta;
        }
        previous = here;
    }
    let mut bits = 0u32;
    for point in &road.shape {
        put_varint(out, (point.elevation.to_bits() ^ bits).into());
        bits = point.elevation.to_bits();
    }
    Ok(())
}

/// Fills `road` from its geometry record `bytes`, with the box `bounds` of its level-1 cell. The
/// fields that the record does not have get their default. Returns the closure field.
pub fn decode_road(bytes: &[u8], bounds: [i32; 4], road: &mut Road) -> Result<u32> {
    let mut at = 0;
    let way = i64::try_from(varint(bytes, &mut at)?).map_err(|_| invalid("a way ID above 63 bits"))?;
    let (use_, scales) = (byte(bytes, at)?, byte(bytes, at + 1)?);
    at += 2;
    let surface =
        [Surface::Unknown, Surface::Paved, Surface::Compacted, Surface::Gravel, Surface::Dirt, Surface::Rough]
            .get(usize::from(use_ & 7))
            .copied()
            .ok_or_else(|| invalid("a bad surface"))?;
    let difficulty = match scales & 15 {
        15 => 255,
        scale @ 0..=6 => scale,
        _ => return Err(invalid("a bad MTB scale")),
    };
    let hiking_difficulty = match scales >> 4 {
        15 => None,
        14 => Some(255),
        scale @ 0..=6 => Some(scale),
        _ => return Err(invalid("a bad SAC scale")),
    };
    let length_m = small(varint(bytes, &mut at)?)?;
    let closure = small(varint(bytes, &mut at)?)?;
    let points = varint(bytes, &mut at)?;
    // Each point takes at least three bytes, so the count is bounded by the record.
    if points < 2 || points > bytes.len() as u64 {
        return Err(invalid("a bad point count"));
    }
    let mut shape = std::mem::take(&mut road.shape);
    shape.clear();
    let (mut previous, mut step) = ([i64::from(bounds[1]), i64::from(bounds[0])], [0i64; 2]);
    for k in 0..points {
        let mut here = [0; 2];
        for axis in 0..2 {
            let value = signed(bytes, &mut at)?;
            let delta = if k < 2 { value } else { value + step[axis] };
            here[axis] = previous[axis] + delta;
            step[axis] = delta;
        }
        let [lat, lon] = here.map(|value| i32::try_from(value).map_err(|_| invalid("a point outside 32 bits")));
        shape.push(Point { lat: lat?, lon: lon?, elevation: 0.0 });
        previous = here;
    }
    let mut bits = 0u32;
    for point in &mut shape {
        bits ^= small(varint(bytes, &mut at)?)?;
        point.elevation = f32::from_bits(bits);
    }
    if at != bytes.len() {
        return Err(invalid("trailing bytes in a road record"));
    }
    *road =
        Road { way, length_m, surface, access: use_ >> 3 & 7, difficulty, hiking_difficulty, shape, ..Road::default() };
    Ok(closure)
}

/// Appends closure set `set`: a count and its entries.
pub fn encode_set(set: &[(u8, Closure)], out: &mut Vec<u8>) -> std::result::Result<(), String> {
    out.push(u8::try_from(set.len()).map_err(|_| "a closure set has more than 255 entries")?);
    for (modes, closure) in set {
        out.push(*modes);
        out.push(KINDS.iter().position(|kind| *kind == closure.kind).expect("each kind is listed") as u8);
        put_varint(out, closure.condition.len() as u64);
        out.extend(closure.condition.as_bytes());
    }
    Ok(())
}

pub fn decode_set(bytes: &[u8]) -> Result<Set> {
    let count = byte(bytes, 0)?;
    let mut at = 1;
    let mut set = Vec::with_capacity(count.into());
    for _ in 0..count {
        let (modes, kind) = (byte(bytes, at)?, byte(bytes, at + 1)?);
        at += 2;
        let kind = *KINDS.get(usize::from(kind)).ok_or_else(|| invalid("a bad closure kind"))?;
        let length = varint(bytes, &mut at)? as usize;
        let text = bytes.get(at..at.saturating_add(length)).ok_or_else(|| invalid("a short closure condition"))?;
        let condition = std::str::from_utf8(text).map_err(|_| invalid("a closure condition that is not UTF-8"))?.into();
        at += length;
        set.push((modes, Closure { kind, condition }));
    }
    if at != bytes.len() {
        return Err(invalid("trailing bytes in a closure set"));
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NO_ELEVATION;

    #[test]
    fn a_road_record_gives_back_each_field_and_bit() {
        let point = |lat, lon, elevation| Point { lat, lon, elevation };
        let road = Road {
            way: 1 << 40,
            length_m: 12_345,
            surface: Surface::Rough,
            access: 5,
            difficulty: 255,
            hiking_difficulty: Some(255),
            shape: vec![
                point(47_000_000, 8_000_000, NO_ELEVATION),
                point(46_999_990, 8_000_015, 412.25),
                point(47_000_100, 7_999_000, -0.0),
            ],
            ..Road::default()
        };
        let bounds = [7_999_000, 46_999_000, 8_100_000, 47_100_000];
        let mut bytes = Vec::new();
        encode_road(&road, 3, bounds, &mut bytes).unwrap();
        let mut decoded = Road { class: 4, structure: true, ..Road::default() };
        assert_eq!(decode_road(&bytes, bounds, &mut decoded).unwrap(), 3);
        let bits = |road: &Road| road.shape.iter().map(|p| (p.lat, p.lon, p.elevation.to_bits())).collect::<Vec<_>>();
        assert_eq!(bits(&decoded), bits(&road));
        assert_eq!((decoded.class, decoded.structure, decoded.hiking_difficulty), (0, false, Some(255)));
        assert!(decode_road(&bytes[..bytes.len() - 1], bounds, &mut decoded).is_err());
        assert!(encode_road(&Road { difficulty: 7, ..road }, 0, bounds, &mut bytes).is_err());
    }
}
