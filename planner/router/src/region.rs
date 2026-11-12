//! The request data of an install of format 10: snapping, the roads, their costs, the turns into
//! them and their closures. It reads the parts through a `Host`, keeps no cache and decodes
//! nothing at open.

use crate::{
    cells::{self, meets, Directory, Geometry, Group, Header, Level1, Reference, Topology, GEOMETRY, TOPOLOGY},
    closures::Closure,
    host::{Host, Part, Span},
    model::{Point, Profile, Road},
    package::{digest, Departure, Endpoint},
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

/// An installed download cell.
#[derive(Clone, Copy)]
struct Cell {
    record: u32,
    id: u32,
    first_road: u32,
    roads: u32,
}

/// An installed profile, its group and its place in the group.
struct Installed {
    profile: Profile,
    group: usize,
    place: u32,
}

/// What all workers share. It grows with the installed download cells, not with the roads.
struct Shared<H> {
    host: H,
    directory: Vec<u8>,
    header: Header,
    downloads: usize,
    identity: String,
    info: Info,
    profiles: Vec<Installed>,
    /// In ascending `first_road`, and so in ascending ID.
    cells: Vec<Cell>,
}

/// A road in the topology part of its download cell.
struct Place<'a> {
    cell: Cell,
    topology: Topology<'a>,
    /// The level-1 record and the index of the road in it.
    index: u32,
    level1: Level1,
    road: u32,
}

/// The data of one worker. A host opens a new `Region` after an install that removed cells.
pub struct Region<H> {
    shared: Arc<Shared<H>>,
    road: RefCell<Road>,
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
        let mut cells = Vec::new();
        for &record in records.iter().filter(|&&record| (record as usize) < downloads) {
            let cell = view.cell(record as usize)?;
            cells.push(Cell { record, id: cell.id, first_road: cell.first_road, roads: cell.roads });
        }
        cells.sort_unstable_by_key(|cell| cell.record);
        let identity = digest(&directory);
        let shared = Shared { host, directory, header, downloads, identity, info, profiles, cells };
        Ok(Self { shared: Arc::new(shared), road: RefCell::default() })
    }

    /// A handle for another worker.
    pub fn fork(&self) -> Self {
        Self { shared: Arc::clone(&self.shared), road: RefCell::default() }
    }

    pub fn host(&self) -> &H {
        &self.shared.host
    }

    /// The SHA-256 of `directory.bin` in hex.
    pub fn identity(&self) -> &str {
        &self.shared.identity
    }
    pub fn region(&self) -> &str {
        &self.shared.info.region
    }
    pub fn bounds(&self) -> [f64; 4] {
        self.shared.info.bounds
    }
    pub fn attribution(&self) -> &str {
        &self.shared.info.attribution
    }
    pub fn warnings(&self) -> &[String] {
        &self.shared.info.warnings
    }
    /// The profiles of the installed groups.
    pub fn profiles(&self) -> Vec<&str> {
        self.shared.profiles.iter().map(|installed| installed.profile.name.as_str()).collect()
    }
    pub fn profile(&self, name: &str) -> Result<&Profile> {
        Ok(&self.shared.installed(name)?.profile)
    }

    pub fn snap(&self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
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
            let geometry = Geometry::new(shared.part(cell.record, GEOMETRY)?)?;
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
            let geometry = Geometry::new(shared.part(cell.record, GEOMETRY)?)?;
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
            let group = Group::new(shared.part(place.cell.record, 2 + installed.group)?)?;
            if !group.allowed(&group.level1(place.index, &place.level1)?, installed.place, place.road)? {
                return Ok(None);
            }
            let mut road = self.road.borrow_mut();
            shared.road(&place, &mut road)?;
            Ok(snap::project(id, &road, point, policy.radius_m))
        })
    }

    /// Applies `f` to a road, decoded into the road of this handle.
    pub fn with_road<T>(&self, road: u32, f: impl FnOnce(&Road) -> T) -> Result<T> {
        let mut decoded = self.road.borrow_mut();
        self.shared.road(&self.shared.place(road)?, &mut decoded)?;
        Ok(f(&decoded))
    }

    /// The closures that concern `mode` on `road`, or `None`.
    pub fn closing(&self, road: u32, mode: u8) -> Result<Option<Vec<Closure>>> {
        let (geometry, closure) = self.shared.road(&self.shared.place(road)?, &mut self.road.borrow_mut())?;
        let Some(set) = closure.checked_sub(1) else { return Ok(None) };
        let closures: Vec<Closure> =
            geometry.set(set)?.into_iter().filter(|(modes, _)| modes & mode != 0).map(|(_, closure)| closure).collect();
        Ok((!closures.is_empty()).then_some(closures))
    }

    /// The cost curve of `road` and the roads from which a turn enters it, with the turn cost, in
    /// ascending road ID. A turn from a cell that is not installed is not a turn.
    pub fn endpoint(&self, metric: &str, road: u32) -> Result<Endpoint> {
        let shared = &*self.shared;
        let installed = shared.installed(metric)?;
        let place = shared.place(road)?;
        let group = Group::new(shared.part(place.cell.record, 2 + installed.group)?)?;
        let class = group.class(&group.level1(place.index, &place.level1)?, place.road)?;
        let Some(basis) = group.basis(class, installed.place)? else {
            return Ok(Endpoint { cost: None, departures: Vec::new() });
        };
        let cost = {
            let mut decoded = self.road.borrow_mut();
            shared.road(&place, &mut decoded)?;
            basis.compile(&decoded, &installed.profile).map_err(Error::InvalidData)?
        };
        Ok(Endpoint { cost: Some(cost), departures: shared.departures(&place, road, installed.group)? })
    }

    /// Tells the host that the engine reads the geometry blocks of these roads next.
    pub fn announce(&self, roads: &[u32]) {
        let shared = &*self.shared;
        let mut spans: Vec<Span> = roads
            .iter()
            .filter_map(|&road| {
                let place = shared.place(road).ok()?;
                let geometry = Geometry::new(shared.part(place.cell.record, GEOMETRY).ok()?).ok()?;
                let cell = geometry.level1(place.index).ok()?;
                let offsets = (cell.roads + 1).checked_mul(cell.offset_width.into())?;
                let bytes = offsets.checked_add(cell.bytes)?;
                Some(Span { part: Part { record: place.cell.record, part: GEOMETRY as u8 }, at: cell.block, bytes })
            })
            .collect();
        spans.sort_unstable();
        spans.dedup();
        shared.host.announce(&spans);
    }
}

impl<H: Host> Shared<H> {
    fn directory(&self) -> Directory<'_> {
        Directory { bytes: &self.directory, header: self.header, downloads: self.downloads }
    }

    fn installed(&self, name: &str) -> Result<&Installed> {
        self.profiles
            .iter()
            .find(|installed| installed.profile.name == name)
            .ok_or_else(|| Error::InvalidRequest(format!("Profile {name} is not installed")))
    }

    /// The whole of part `part` of record `record`.
    fn part(&self, record: u32, part: usize) -> Result<&[u8]> {
        let bytes = self.directory().part(record as usize, part)?.bytes;
        self.host.range(Span { part: Part { record, part: part as u8 }, at: 0, bytes })
    }

    /// The installed download cell with ID `id`.
    fn cell(&self, id: u32) -> Option<Cell> {
        self.cells.binary_search_by_key(&id, |cell| cell.id).ok().map(|k| self.cells[k])
    }

    fn place(&self, road: u32) -> Result<Place<'_>> {
        let k = self.cells.partition_point(|cell| cell.first_road <= road);
        let cell = k.checked_sub(1).map(|k| self.cells[k]).filter(|cell| road - cell.first_road < cell.roads);
        let cell = cell.ok_or_else(|| Error::MissingRegion(format!("Road {road} is not installed")))?;
        let topology = Topology::new(self.part(cell.record, TOPOLOGY)?)?;
        let local = road - cell.first_road;
        // The last level-1 cell whose first road is at or before the road.
        let (mut low, mut high) = (0, topology.cells);
        while low < high {
            let middle = low + (high - low) / 2;
            if topology.level1(middle)?.first_road <= local {
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
        let geometry = Geometry::new(self.part(place.cell.record, GEOMETRY)?)?;
        let closure = geometry.road(&geometry.level1(place.index)?, place.road, road)?;
        Ok((geometry, closure))
    }

    /// The turns into the road of `place`, which has ID `road`, with the costs of group `group`.
    fn departures(&self, place: &Place, road: u32, group: usize) -> Result<Vec<Departure>> {
        // The level-1 cell of the departure junction, and the target number of the road there.
        let (cell, topology, index, level1, target) = if place.road >= u32::from(place.level1.entering) {
            (place.cell, place.topology, place.index, place.level1, place.road)
        } else {
            let id = place.topology.departure(&place.level1, place.road)?;
            let Some(cell) = self.cell(id & !self.directory().below(self.header.download_level)) else {
                return Ok(Vec::new());
            };
            let topology = Topology::new(self.part(cell.record, TOPOLOGY)?)?;
            let index = search(topology.cells, |k| Ok(topology.level1(k)?.id.cmp(&id)))?
                .ok_or_else(|| invalid("a departure in a level-1 cell that its download cell lacks"))?;
            let level1 = topology.level1(index)?;
            let reference = Reference { cell: place.cell.id, index: road - place.cell.first_road };
            let k = search(level1.leaving.into(), |k| Ok(topology.leaving(&level1, k)?.cmp(&reference)))?
                .ok_or_else(|| invalid("an entering road that the leaving list of its departure lacks"))?;
            (cell, topology, index, level1, level1.roads + k)
        };
        let costs = Group::new(self.part(cell.record, 2 + group)?)?;
        let costs_cell = costs.level1(index, &level1)?;
        let mut departures = Vec::new();
        for from in 0..level1.roads {
            let turns = topology.turns(&level1, from)?;
            let found = search(turns.len() as u32, |k| Ok(topology.head(&level1, turns.start + k)?.cmp(&target)))?;
            if let Some(penalty) = found.map(|k| costs.turn(&costs_cell, turns.start + k)).transpose()?.flatten() {
                let state = cell.first_road + level1.first_road + from;
                departures.push(Departure { state, penalty: penalty.into() });
            }
        }
        Ok(departures)
    }
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
