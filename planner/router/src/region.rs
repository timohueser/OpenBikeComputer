//! The request data of an install of format 10: snapping, the roads, their costs, the turns into
//! them, their closures, and the cells with a cost table. It reads the parts through a `Host`, keeps
//! no cache and decodes nothing at open.

use crate::{
    cells::{
        self, meets, values, Directory, Geometry, Group, GroupCell, Header, Level1, Reference, Table, Topology, Upper,
        GEOMETRY, TOPOLOGY,
    },
    closures::Closure,
    data::RoutingData,
    host::{Host, Part, Span},
    model::{Point, Profile, Road},
    overlay::{self, Found, Query, Stats, Workspace, SEARCH_BYTES},
    package::{digest, Departure, Endpoint},
    router::Control,
    snap::{self, Candidates, Policy},
    Error, Result,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeSet, sync::Arc};

fn invalid(what: impl std::fmt::Display) -> Error {
    Error::InvalidData(format!("Invalid route cell data: {what}"))
}

/// The fields of `package.json` that a request reads.
#[derive(Deserialize)]
struct Info {
    region: String,
    bounds: [f64; 4],
    attribution: String,
    warnings: Vec<String>,
    profiles: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    group: u8,
    profile: Profile,
}

/// An installed cell: a download cell or an upper cell.
#[derive(Clone, Copy)]
pub(crate) struct Cell {
    pub record: u32,
    level: u8,
    pub id: u32,
    pub first_road: u32,
    roads: u32,
    /// Its place in `Shared::sizes`.
    slot: u32,
}

/// An installed profile, its group and its place in the group.
pub(crate) struct Installed {
    profile: Profile,
    pub group: usize,
    pub place: u32,
}

/// What all workers share. It grows with the installed download cells, not with the roads.
pub(crate) struct Shared<H> {
    host: H,
    directory: Vec<u8>,
    header: Header,
    downloads: usize,
    identity: String,
    info: Info,
    profiles: Vec<Installed>,
    /// The download cells, in ascending `first_road`, and so in ascending ID.
    cells: Vec<Cell>,
    /// The upper cells, in the order of level and ID.
    uppers: Vec<Cell>,
    /// The size of each part of each installed cell, from the directory.
    sizes: Vec<u32>,
}

/// A level-1 cell in the topology part of its download cell.
#[derive(Clone, Copy)]
pub(crate) struct Home<'a> {
    pub cell: Cell,
    pub topology: Topology<'a>,
    /// The level-1 record.
    pub index: u32,
    pub level1: Level1,
}

/// A road in the topology part of its download cell.
pub(crate) struct Place<'a> {
    pub cell: Cell,
    pub topology: Topology<'a>,
    /// The level-1 record and the index of the road in it.
    pub index: u32,
    pub level1: Level1,
    pub road: u32,
}

/// The level-1 cell of the departure junction of a road, and the target number of the road in it:
/// its index, or `roads` plus its place in `leaving`.
pub(crate) struct Junction<'a> {
    pub cell: Cell,
    pub topology: Topology<'a>,
    pub index: u32,
    pub level1: Level1,
    pub target: u32,
}

impl<'a> Place<'a> {
    pub(crate) fn home(&self) -> Home<'a> {
        Home { cell: self.cell, topology: self.topology, index: self.index, level1: self.level1 }
    }

    /// The reference of the road.
    pub(crate) fn reference(&self) -> Reference {
        Reference { cell: self.cell.id, index: self.level1.first_road + self.road }
    }
}

impl<'a> Junction<'a> {
    pub(crate) fn home(&self) -> Home<'a> {
        Home { cell: self.cell, topology: self.topology, index: self.index, level1: self.level1 }
    }
}

/// The two road lists of a cell.
#[derive(Clone, Copy)]
enum Lists {
    /// A level-1 cell of the download cell with this ID: its entering roads are its first roads.
    Level1(u32, Level1),
    Upper(Upper),
}

/// A cell with a cost table: its entering and its leaving roads, and the table of one profile.
pub(crate) struct Closed<'a> {
    pub table: Table<'a>,
    topology: Topology<'a>,
    lists: Lists,
}

impl Closed<'_> {
    /// The row of the entering road `road`. A road that the list lacks is `InvalidData`.
    pub(crate) fn row(&self, road: Reference) -> Result<u32> {
        let row = match self.lists {
            Lists::Level1(cell, level1) => {
                let row = road.index.checked_sub(level1.first_road).filter(|&row| row < level1.entering.into());
                row.filter(|_| road.cell == cell)
            }
            Lists::Upper(upper) => {
                search(upper.entering, |k| Ok(self.topology.reference(upper.entering_at.into(), k)?.cmp(&road)))?
            }
        };
        row.ok_or_else(|| invalid("a road that the entering list of its cell lacks"))
    }

    /// The column of the leaving road `road`. A road that the list lacks is `InvalidData`.
    pub(crate) fn column(&self, road: Reference) -> Result<u32> {
        let column = match self.lists {
            Lists::Level1(_, level1) => {
                search(level1.leaving.into(), |k| Ok(self.topology.leaving(&level1, k)?.cmp(&road)))?
            }
            Lists::Upper(upper) => {
                search(upper.leaving, |k| Ok(self.topology.reference(upper.leaving_at.into(), k)?.cmp(&road)))?
            }
        };
        column.ok_or_else(|| invalid("a road that the leaving list of its cell lacks"))
    }

    /// The entering road of `row`, which is below the rows of the table.
    pub(crate) fn entering(&self, row: u32) -> Result<Reference> {
        match self.lists {
            Lists::Level1(cell, level1) => Ok(Reference { cell, index: level1.first_road + row }),
            Lists::Upper(upper) => self.topology.reference(upper.entering_at.into(), row),
        }
    }

    /// The leaving road of `column`, which is below the columns of the table.
    pub(crate) fn leaving(&self, column: u32) -> Result<Reference> {
        match self.lists {
            Lists::Level1(_, level1) => self.topology.leaving(&level1, column),
            Lists::Upper(upper) => self.topology.reference(upper.leaving_at.into(), column),
        }
    }
}

/// The run-time IDs of road references. The lists ascend, so it keeps the last download cell.
pub(crate) struct Ids<'a, H> {
    shared: &'a Shared<H>,
    last: Option<Cell>,
}

impl<H: Host> Ids<'_, H> {
    /// The run-time ID of `reference`; `None` when its cell is not installed.
    pub(crate) fn get(&mut self, reference: Reference) -> Option<u32> {
        if self.last.is_none_or(|cell| cell.id != reference.cell) {
            self.last = Some(self.shared.cell(reference.cell)?);
        }
        self.last.filter(|cell| reference.index < cell.roads).map(|cell| cell.first_road + reference.index)
    }
}

/// The data of one worker. A host opens a new `Region` after an install that removed cells.
pub struct Region<H> {
    shared: Arc<Shared<H>>,
    road: RefCell<Road>,
    workspace: Workspace,
    /// The search limit: the most bytes of the labels, queues and reverse turns of one search.
    budget: usize,
}

impl<H: Host> Region<H> {
    /// Opens the install of `directory` (`directory.bin`), `info` (`package.json`) and `install`
    /// (`install.bin`), whose parts `host` gives.
    pub fn open(host: H, directory: Vec<u8>, info: Vec<u8>, install: &[u8]) -> Result<Self> {
        let view = Directory::new(&directory)?;
        let (header, downloads) = (view.header, view.downloads());
        let (groups, records) = cells::marker(&directory, install)?;
        if <[u8; 32]>::from(Sha256::digest(&info)) != header.info {
            return Err(Error::InvalidData("package.json is not that of the directory".into()));
        }
        let info: Info = serde_json::from_slice(&info).map_err(|e| invalid(format!("package.json: {e}")))?;
        let mut profiles: Vec<Installed> = Vec::new();
        for (k, entry) in info.profiles.iter().enumerate() {
            let group = usize::from(entry.group);
            if group >= usize::from(header.groups) {
                return Err(invalid("a profile of a group that the directory does not have"));
            }
            let place = info.profiles[..k].iter().filter(|other| other.group == entry.group).count() as u32;
            if groups >> group & 1 != 0 {
                profiles.push(Installed { profile: entry.profile.clone(), group, place });
            }
        }
        let mut records = records;
        records.sort_unstable();
        let (mut cells, mut uppers, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
        for (slot, &record) in records.iter().enumerate() {
            let cell = view.cell(record as usize)?;
            let (level, id, first_road, roads, slot) = (cell.level, cell.id, cell.first_road, cell.roads, slot as u32);
            let list = if (record as usize) < downloads { &mut cells } else { &mut uppers };
            list.push(Cell { record, level, id, first_road, roads, slot });
            sizes.extend(cell.parts.iter().map(|part| part.bytes));
        }
        let identity = digest(&directory);
        let shared = Shared { host, directory, header, downloads, identity, info, profiles, cells, uppers, sizes };
        Ok(Self { shared: Arc::new(shared), road: RefCell::default(), workspace: Workspace::default(), budget: 0 })
    }

    /// A handle for another worker.
    pub fn fork(&self) -> Self {
        let workspace = Workspace::default();
        Self { shared: Arc::clone(&self.shared), road: RefCell::default(), workspace, budget: self.budget }
    }

    pub fn host(&self) -> &H {
        &self.shared.host
    }

    /// The counts of the searches since the last call.
    pub fn take_stats(&self) -> Stats {
        self.workspace.take_stats()
    }
}

impl<H: Host> RoutingData for Region<H> {
    /// The SHA-256 of `directory.bin` in hex.
    fn identity(&self) -> &str {
        &self.shared.identity
    }
    fn region(&self) -> &str {
        &self.shared.info.region
    }
    fn bounds(&self) -> [f64; 4] {
        self.shared.info.bounds
    }
    fn attribution(&self) -> &str {
        &self.shared.info.attribution
    }
    fn warnings(&self) -> &[String] {
        &self.shared.info.warnings
    }
    /// The profiles of the installed groups.
    fn profiles(&self) -> Vec<&str> {
        self.shared.profiles.iter().map(|installed| installed.profile.name.as_str()).collect()
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        Ok(&self.shared.installed(name)?.profile)
    }

    fn snap(&self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        let shared = &*self.shared;
        let installed = shared.installed(metric)?;
        let window = snap::window(point, policy.radius_m).map_err(Error::InvalidRequest)?;
        let directory = shared.directory();
        let mut roads = BTreeSet::new();
        let limit = |roads: &BTreeSet<u32>| if roads.len() > 100_000 { Err(Error::Limit) } else { Ok(()) };
        for record in directory.meeting(window)? {
            let found = shared.cells.binary_search_by_key(&(record as u32), |cell| cell.record);
            let cell = shared.cells[found.map_err(|_| {
                Error::MissingRegion(format!("The roads near point {},{} are not installed", point.lat, point.lon))
            })?];
            let geometry = Geometry::new(shared.part(&cell, GEOMETRY)?)?;
            let mut first = cell.first_road;
            for index in 0..geometry.cells {
                let level1 = geometry.level1(index)?;
                let end = first.checked_add(level1.roads).ok_or_else(|| invalid("a road count above 32 bits"))?;
                if meets(level1.bounds, window) {
                    roads.extend(first..end);
                }
                first = end;
            }
            limit(&roads)?;
        }
        for cell in &shared.cells {
            let long = directory.boxes(cell.record as usize)?.1;
            if long == [0; 4] || !meets(long, window) {
                continue;
            }
            let geometry = Geometry::new(shared.part(cell, GEOMETRY)?)?;
            for k in 0..geometry.long {
                let (index, bounds) = geometry.long(k)?;
                if meets(bounds, window) {
                    roads.insert(cell.first_road.checked_add(index).ok_or_else(|| invalid("a long road"))?);
                }
            }
            limit(&roads)?;
        }
        snap::candidates(roads, policy, |id| {
            let place = shared.place(id)?;
            let group = Group::new(shared.part(&place.cell, 2 + installed.group)?)?;
            if !group.allowed(&group.level1(place.index, &place.level1)?, installed.place, place.road)? {
                return Ok(None);
            }
            let mut road = self.road.borrow_mut();
            shared.road(&place, &mut road)?;
            Ok(snap::project(id, &road, point, policy.radius_m))
        })
    }

    /// Applies `f` to a road, decoded into the road of this handle.
    fn with_road<T>(&self, road: u32, f: impl FnOnce(&Road) -> T) -> Result<T> {
        let mut decoded = self.road.borrow_mut();
        self.shared.road(&self.shared.place(road)?, &mut decoded)?;
        Ok(f(&decoded))
    }

    /// The closures that concern `mode` on `road`, or `None`.
    fn closing(&self, road: u32, mode: u8) -> Result<Option<Vec<Closure>>> {
        let (geometry, closure) = self.shared.road(&self.shared.place(road)?, &mut self.road.borrow_mut())?;
        let Some(set) = closure.checked_sub(1) else { return Ok(None) };
        let closures: Vec<Closure> =
            geometry.set(set)?.into_iter().filter(|(modes, _)| modes & mode != 0).map(|(_, closure)| closure).collect();
        Ok((!closures.is_empty()).then_some(closures))
    }

    /// The cost curve of `road` and the roads from which a turn enters it, with the turn cost, in
    /// ascending road ID. A turn from a cell that is not installed is not a turn.
    fn endpoint(&self, metric: &str, road: u32) -> Result<Endpoint> {
        let shared = &*self.shared;
        let installed = shared.installed(metric)?;
        let place = shared.place(road)?;
        let group = Group::new(shared.part(&place.cell, 2 + installed.group)?)?;
        let group_cell = group.level1(place.index, &place.level1)?;
        let class = group.class(&group_cell, place.road)?;
        let Some(basis) = group.basis(class, installed.place)? else {
            return Ok(Endpoint { cost: None, departures: Vec::new() });
        };
        let cost = {
            let mut decoded = self.road.borrow_mut();
            shared.road(&place, &mut decoded)?;
            basis.compile(&decoded, &installed.profile).map_err(Error::InvalidData)?
        };
        let departures = shared.departures(&place, installed.group)?;
        Ok(Endpoint { cost: Some(cost), departures })
    }

    /// Tells the host that the engine reads the geometry blocks of these roads next.
    fn announce(&self, roads: &[u32]) {
        let shared = &*self.shared;
        let spans = roads
            .iter()
            .filter_map(|&road| {
                let place = shared.place(road).ok()?;
                let geometry = Geometry::new(shared.part(&place.cell, GEOMETRY).ok()?).ok()?;
                let cell = geometry.level1(place.index).ok()?;
                let offsets = (cell.roads + 1).checked_mul(cell.offset_width.into())?;
                let bytes = offsets.checked_add(cell.bytes)?;
                Some(Span { part: Part { record: place.cell.record, part: GEOMETRY as u8 }, at: cell.block, bytes })
            })
            .collect();
        shared.announce(spans);
    }

    fn search(&mut self, metric: &str, query: Query<'_>) -> Result<Option<Found>> {
        let installed = self.shared.installed(metric)?;
        let limit = self.budget.min(query.heap_bytes);
        overlay::search(&self.shared, &mut self.workspace, installed, query, limit)
    }

    fn unpack(&mut self, metric: &str, legs: &[&Found], control: &Control<'_>) -> Result<Vec<Vec<u32>>> {
        let installed = self.shared.installed(metric)?;
        let limit = self.budget.min(control.max_heap_bytes);
        overlay::unpack(&self.shared, &mut self.workspace, installed, legs, control, limit)
    }

    /// The search limit.
    fn memory_budget(&self) -> usize {
        self.budget
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }
    fn default_budget(&self) -> usize {
        SEARCH_BYTES
    }
}

impl<H: Host> Shared<H> {
    fn directory(&self) -> Directory<'_> {
        Directory { bytes: &self.directory, header: self.header, downloads: self.downloads }
    }

    pub(crate) fn installed(&self, name: &str) -> Result<&Installed> {
        self.profiles
            .iter()
            .find(|installed| installed.profile.name == name)
            .ok_or_else(|| Error::InvalidRequest(format!("Profile {name} is not installed")))
    }

    /// The size of part `part` of `cell`; 0 for a part that the cell does not have.
    fn size(&self, cell: &Cell, part: usize) -> Result<u32> {
        let parts = 2 + usize::from(self.header.groups);
        let size = self.sizes.get(cell.slot as usize * parts + part).filter(|_| part < parts);
        size.copied().ok_or_else(|| invalid("a part outside the directory"))
    }

    /// The whole of part `part` of `cell`.
    pub(crate) fn part(&self, cell: &Cell, part: usize) -> Result<&[u8]> {
        let bytes = self.size(cell, part)?;
        self.host.range(Span { part: Part { record: cell.record, part: part as u8 }, at: 0, bytes })
    }

    /// The engine reads these spans next. Each span goes to the host once.
    pub(crate) fn announce(&self, mut spans: Vec<Span>) {
        spans.sort_unstable();
        spans.dedup();
        self.host.announce(&spans);
    }

    /// The highest level at which two level-1 cells whose IDs differ in the bits `differ` lie in two
    /// cells; 0 for one level-1 cell.
    pub(crate) fn apart(&self, differ: u32) -> u8 {
        let directory = self.directory();
        (1..=self.header.levels).rev().find(|&level| differ & !directory.below(level) != 0).unwrap_or(0)
    }

    /// The lists and the table, for the profile `installed`, of the cell of `level` that holds the
    /// level-1 cell `home`. `None` for a cell that the search treats as open: its slot has 0 bytes,
    /// or it is an upper cell that is not installed or has no group part.
    pub(crate) fn closed<'a>(
        &'a self,
        home: &Home<'a>,
        level: u8,
        installed: &Installed,
    ) -> Result<Option<Closed<'a>>> {
        let (group, profile) = (2 + installed.group, installed.place);
        if level == 1 {
            let (costs, level1) = (Group::new(self.part(&home.cell, group)?)?, home.level1);
            let table = costs.table(profile, home.index, level1.entering.into(), level1.leaving.into())?;
            let lists = Lists::Level1(home.cell.id, level1);
            return Ok(table.map(|table| Closed { table, topology: home.topology, lists }));
        }
        let key = (level, home.level1.id & !self.directory().below(level));
        let (cell, topology, record) = if level <= self.header.download_level {
            let topology = home.topology;
            let found = search(topology.upper, |k| {
                let upper = topology.upper(k)?;
                Ok((upper.level, upper.id).cmp(&key))
            })?;
            (home.cell, topology, found.ok_or_else(|| invalid("a download cell without the record of its cell"))?)
        } else {
            let Ok(k) = self.uppers.binary_search_by_key(&key, |cell| (cell.level, cell.id)) else { return Ok(None) };
            let cell = self.uppers[k];
            if self.size(&cell, group)? == 0 {
                return Ok(None);
            }
            (cell, Topology::new(self.part(&cell, TOPOLOGY)?)?, 0)
        };
        let upper = topology.upper(record)?;
        if (upper.level, upper.id) != key {
            return Err(invalid("an upper cell with the record of another cell"));
        }
        let costs = Group::new(self.part(&cell, group)?)?;
        let table = costs.table(profile, costs.cells + record, upper.entering, upper.leaving)?;
        Ok(table.map(|table| Closed { table, topology, lists: Lists::Upper(upper) }))
    }

    /// The run-time IDs of the roads of the cell of `level`, from 2, that holds the level-1 cell
    /// `home`.
    pub(crate) fn roads(&self, home: &Home, level: u8) -> Result<std::ops::Range<u32>> {
        let below = self.directory().below(level);
        let id = home.level1.id & !below;
        let cell = match level.cmp(&self.header.download_level) {
            std::cmp::Ordering::Less => {
                let topology = &home.topology;
                let first = |k: u32| match k < topology.cells {
                    true => topology.first_road(k),
                    false => Ok(topology.roads),
                };
                let start = partition(topology.cells, |k| Ok(topology.level1(k)?.id < id))?;
                let end = partition(topology.cells, |k| Ok(topology.level1(k)?.id <= id | below))?;
                return Ok(home.cell.first_road + first(start)?..home.cell.first_road + first(end)?);
            }
            std::cmp::Ordering::Equal => home.cell,
            std::cmp::Ordering::Greater => {
                let found = self.uppers.binary_search_by_key(&(level, id), |cell| (cell.level, cell.id));
                self.uppers[found.map_err(|_| invalid("a path step in an upper cell that is not installed"))?]
            }
        };
        Ok(cell.first_road..cell.first_road + cell.roads)
    }

    /// Adds the spans that the unpacking of a step of `level` in the cell of `home` reads: for
    /// level 1 the topology block and the group block of the level-1 cell, else the two lists and
    /// the table of each child of the cell.
    pub(crate) fn spans(&self, home: &Home, level: u8, installed: &Installed, spans: &mut Vec<Span>) -> Result<()> {
        let (group, profile) = (2 + installed.group, installed.place);
        let mut add = |cell: &Cell, part: usize, (at, bytes): (u32, u32)| {
            spans.push(Span { part: Part { record: cell.record, part: part as u8 }, at, bytes });
        };
        if level == 1 {
            add(&home.cell, TOPOLOGY, home.topology.block(&home.level1)?);
            let costs = Group::new(self.part(&home.cell, group)?)?;
            add(&home.cell, group, costs.level1(home.index, &home.level1)?.span()?);
            return Ok(());
        }
        let (child, mask) = (level - 1, !self.directory().below(level));
        let id = home.level1.id & mask;
        if child > self.header.download_level {
            let start = self.uppers.partition_point(|cell| (cell.level, cell.id) < (child, id));
            for cell in self.uppers[start..].iter().take_while(|cell| cell.level == child && cell.id & mask == id) {
                add(cell, TOPOLOGY, (0, self.size(cell, TOPOLOGY)?));
                add(cell, group, Group::new(self.part(cell, group)?)?.slot(profile, 0)?);
            }
            return Ok(());
        }
        // The download cells of an upper cell, or the one download cell that holds the cell.
        let downloads = match level > self.header.download_level {
            true => {
                let rest = &self.cells[self.cells.partition_point(|cell| cell.id < id)..];
                &rest[..rest.iter().take_while(|cell| cell.id & mask == id).count()]
            }
            false => std::slice::from_ref(&home.cell),
        };
        for cell in downloads {
            let (topology, costs) = (Topology::new(self.part(cell, TOPOLOGY)?)?, Group::new(self.part(cell, group)?)?);
            if child == 1 {
                for k in partition(topology.cells, |k| Ok(topology.level1(k)?.id < id))?..topology.cells {
                    let level1 = topology.level1(k)?;
                    if level1.id & mask != id {
                        break;
                    }
                    add(cell, TOPOLOGY, topology.leaving_list(&level1)?);
                    add(cell, group, costs.slot(profile, k)?);
                }
                continue;
            }
            let first = partition(topology.upper, |k| {
                let upper = topology.upper(k)?;
                Ok((upper.level, upper.id) < (child, id))
            })?;
            for k in first..topology.upper {
                let upper = topology.upper(k)?;
                if upper.level != child || upper.id & mask != id {
                    break;
                }
                let size = cells::REFERENCE as u32;
                add(cell, TOPOLOGY, (upper.entering_at, upper.entering * size));
                add(cell, TOPOLOGY, (upper.leaving_at, upper.leaving * size));
                add(cell, group, costs.slot(profile, costs.cells + k)?);
            }
        }
        Ok(())
    }

    /// The run-time IDs of road references.
    pub(crate) fn ids(&self) -> Ids<'_, H> {
        Ids { shared: self, last: None }
    }

    /// The installed download cell with ID `id`.
    fn cell(&self, id: u32) -> Option<Cell> {
        self.cells.binary_search_by_key(&id, |cell| cell.id).ok().map(|k| self.cells[k])
    }

    /// The installed download cell of road `road`.
    fn cell_of(&self, road: u32) -> Option<Cell> {
        let k = self.cells.partition_point(|cell| cell.first_road <= road);
        k.checked_sub(1).map(|k| self.cells[k]).filter(|cell| road - cell.first_road < cell.roads)
    }

    pub(crate) fn place(&self, road: u32) -> Result<Place<'_>> {
        let cell = self.cell_of(road).ok_or_else(|| Error::MissingRegion(format!("Road {road} is not installed")))?;
        let topology = Topology::new(self.part(&cell, TOPOLOGY)?)?;
        let local = road - cell.first_road;
        // The last level-1 cell whose first road is at or before the road.
        let (mut low, mut high) = (0, topology.cells);
        while low < high {
            let middle = low + (high - low) / 2;
            if topology.first_road(middle)? <= local {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let index = low.checked_sub(1).ok_or_else(|| invalid("a road before the first level-1 cell"))?;
        let level1 = topology.level1(index)?;
        let road = local - level1.first_road;
        if road >= level1.roads {
            return Err(invalid("a road outside its level-1 cell"));
        }
        Ok(Place { cell, topology, index, level1, road })
    }

    /// Decodes the road of `place` into `road`. Returns the geometry part and the closure field.
    fn road(&self, place: &Place, road: &mut Road) -> Result<(Geometry<'_>, u32)> {
        let geometry = Geometry::new(self.part(&place.cell, GEOMETRY)?)?;
        let closure = geometry.road(&geometry.level1(place.index)?, place.road, road)?;
        Ok((geometry, closure))
    }

    /// The departure junction of the road of `place`; `None` when its cell is not installed.
    pub(crate) fn junction<'a>(&'a self, place: &Place<'a>) -> Result<Option<Junction<'a>>> {
        if place.road >= u32::from(place.level1.entering) {
            let (cell, topology, index, level1) = (place.cell, place.topology, place.index, place.level1);
            return Ok(Some(Junction { cell, topology, index, level1, target: place.road }));
        }
        let id = place.topology.departure(&place.level1, place.road)?;
        let Some(cell) = self.cell(id & !self.directory().below(self.header.download_level)) else {
            return Ok(None);
        };
        let topology = Topology::new(self.part(&cell, TOPOLOGY)?)?;
        let index = search(topology.cells, |k| Ok(topology.level1(k)?.id.cmp(&id)))?
            .ok_or_else(|| invalid("a departure in a level-1 cell that its download cell lacks"))?;
        let level1 = topology.level1(index)?;
        let reference = place.reference();
        let k = search(level1.leaving.into(), |k| Ok(topology.leaving(&level1, k)?.cmp(&reference)))?
            .ok_or_else(|| invalid("an entering road that the leaving list of its departure lacks"))?;
        Ok(Some(Junction { cell, topology, index, level1, target: level1.roads + k }))
    }

    /// The group part `group` of `cell` and its record of level-1 cell `index`, `level1`.
    pub(crate) fn group(
        &self,
        cell: &Cell,
        index: u32,
        level1: &Level1,
        group: usize,
    ) -> Result<(Group<'_>, GroupCell<'_>)> {
        let view = Group::new(self.part(cell, 2 + group)?)?;
        Ok((view, view.level1(index, level1)?))
    }

    /// Whether the install has road `road`.
    pub(crate) fn has(&self, road: u32) -> bool {
        self.cell_of(road).is_some()
    }

    /// The turns into the road of `place`, with the costs of group `group`.
    fn departures(&self, place: &Place, group: usize) -> Result<Vec<Departure>> {
        let Some(junction) = self.junction(place)? else { return Ok(Vec::new()) };
        let Junction { cell, topology, index, level1, target } = junction;
        let (costs, costs_cell) = self.group(&cell, index, &level1, group)?;
        let (first, head) = topology.columns(&level1)?;
        let mut departures = Vec::new();
        for (from, turn) in entering((first, level1.first_width), (head, level1.head_width), target)? {
            if let Some(penalty) = costs.turn(&costs_cell, turn)? {
                departures
                    .push(Departure { state: cell.first_road + level1.first_road + from, penalty: penalty.into() });
            }
        }
        Ok(departures)
    }
}

/// The turns of a level-1 cell whose `head` is `target`, each with its road: one pass over the
/// `head` column, and a walk of the `first` column in step. Each column is a slice and its width.
fn entering(first: (&[u8], u8), head: (&[u8], u8), target: u32) -> Result<Vec<(u32, u32)>> {
    let mut ends = values(first.0, first.1);
    if ends.next() != Some(0) {
        return Err(invalid("a turn column that does not start at 0"));
    }
    // The road of the walk, and the end of its turns once read.
    let (mut road, mut end, mut last) = (0, None, 0);
    let mut found = Vec::new();
    for (_, turn) in values(head.0, head.1).zip(0u32..).filter(|&(value, _)| value == target) {
        loop {
            let value = match end {
                Some(value) => value,
                None => {
                    let value = ends.next().ok_or_else(|| invalid("a turn after the last road"))?;
                    if value < last {
                        return Err(invalid("a turn column that goes down"));
                    }
                    (last, end) = (value, Some(value));
                    value
                }
            };
            if turn < value {
                break;
            }
            (road, end) = (road + 1, None);
        }
        found.push((road, turn));
    }
    Ok(found)
}

/// The first `k` below `count` where `before(k)` is false, else `count`, for a `before` that is
/// true up to that `k`.
fn partition(count: u32, before: impl Fn(u32) -> Result<bool>) -> Result<u32> {
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        if before(middle)? {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

/// The `k` below `count` where `compare(k)` is `Equal`, for a `compare` that ascends with `k`.
fn search(count: u32, compare: impl Fn(u32) -> Result<std::cmp::Ordering>) -> Result<Option<u32>> {
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        match compare(middle)? {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => return Ok(Some(middle)),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_of_the_turn_columns_finds_each_road_and_refuses_a_column_that_goes_down() {
        // A level-1 cell with no road: `first` is one 0 and `head` is empty.
        assert_eq!(entering((&[0], 1), (&[], 1), 0).unwrap(), []);
        // Roads 0 and 2 turn into target 5; road 1 has no turn.
        assert_eq!(entering((&[0, 2, 2, 4], 1), (&[5, 1, 0, 5], 1), 5).unwrap(), [(0, 0), (2, 3)]);
        assert!(entering((&[0, 3, 1, 4], 1), (&[5, 5, 5, 5], 1), 5).is_err());
        assert!(entering((&[1, 2], 1), (&[5], 1), 5).is_err());
    }
}
