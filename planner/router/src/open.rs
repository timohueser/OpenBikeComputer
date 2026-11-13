//! Opens a routing folder with the engine of its format: format 9 (pages) or an install of format
//! 10 (cells).
use crate::{
    closures::Closure,
    data::{RoutingData, Selection},
    directory::{self, Files},
    host::Maps,
    install,
    model::{Point, Profile, Road},
    overlay::{Found, Query, Stats},
    package::Endpoint,
    region::Region,
    router::Control,
    snap::{Candidates, Policy},
    Error, Result,
};
use std::path::Path;

/// The data of one worker on either format.
#[allow(clippy::large_enum_variant, reason = "one value for each worker")]
pub enum Opened {
    Pages(Selection<Files>),
    Cells(Region<Maps>),
}

macro_rules! each {
    ($self:expr, $data:ident => $call:expr) => {
        match $self {
            Opened::Pages($data) => $call,
            Opened::Cells($data) => $call,
        }
    };
}

/// A folder with `manifest.json` or `blocks.json` is format 9, also when it holds `directory.bin`.
/// Each other folder is an install of format 10, which `install.bin` marks complete.
pub fn open(path: &Path) -> Result<Opened> {
    if path.join("manifest.json").exists() || path.join("blocks.json").exists() {
        return directory::open(path).map(Opened::Pages);
    }
    cells(path, false).map(Opened::Cells)
}

/// Opens the install of format 10 in `path`; with `silent`, on a host whose `announce` does
/// nothing, which only a measurement uses.
pub fn cells(path: &Path, silent: bool) -> Result<Region<Maps>> {
    if !path.join("directory.bin").is_file() || !path.join(install::MARKER).is_file() {
        return Err(Error::MissingRegion(format!("The routing install in {} is not complete", path.display())));
    }
    let read = |name: &str| {
        let path = path.join(name);
        std::fs::read(&path).map_err(|e| Error::InvalidData(format!("{}: {e}", path.display())))
    };
    let (directory, info, marker) = (read("directory.bin")?, read("package.json")?, read(install::MARKER)?);
    let maps = Maps::new(path, &directory)?;
    Region::open(if silent { maps.silent() } else { maps }, directory, info, &marker)
}

impl Opened {
    /// A handle for another worker.
    pub fn fork(&self) -> Self {
        match self {
            Opened::Pages(data) => Opened::Pages(data.fork()),
            Opened::Cells(data) => Opened::Cells(data.fork()),
        }
    }

    /// The counts of the searches since the last call.
    pub fn take_stats(&self) -> Stats {
        each!(self, data => data.take_stats())
    }

    /// Reads each object of a package of format 9. The installer checked an install.
    pub fn verify(&self) -> Result<()> {
        match self {
            Opened::Pages(data) => data.verify(),
            Opened::Cells(_) => Ok(()),
        }
    }

    /// Loads the costs of `profile` on format 9, before the first request.
    pub fn warm(&self, profile: &str) -> Result<()> {
        match self {
            Opened::Pages(data) => data.prepared(profile).map(drop),
            Opened::Cells(_) => Ok(()),
        }
    }
}

impl RoutingData for Opened {
    fn identity(&self) -> &str {
        each!(self, data => data.identity())
    }
    fn region(&self) -> &str {
        each!(self, data => data.region())
    }
    fn bounds(&self) -> [f64; 4] {
        each!(self, data => data.bounds())
    }
    fn attribution(&self) -> &str {
        each!(self, data => data.attribution())
    }
    fn warnings(&self) -> &[String] {
        each!(self, data => data.warnings())
    }
    fn profiles(&self) -> Vec<&str> {
        each!(self, data => data.profiles())
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        each!(self, data => data.profile(name))
    }
    fn snap(&self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        each!(self, data => data.snap(point, metric, policy))
    }
    fn with_road<T>(&self, road: u32, f: impl FnOnce(&Road) -> T) -> Result<T> {
        each!(self, data => data.with_road(road, f))
    }
    fn closing(&self, road: u32, mode: u8) -> Result<Option<Vec<Closure>>> {
        each!(self, data => data.closing(road, mode))
    }
    fn endpoint(&self, metric: &str, road: u32) -> Result<Endpoint> {
        each!(self, data => data.endpoint(metric, road))
    }
    fn search(&mut self, metric: &str, query: Query<'_>) -> Result<Option<Found>> {
        each!(self, data => data.search(metric, query))
    }
    fn unpack(&mut self, metric: &str, legs: &[&Found], control: &Control<'_>) -> Result<Vec<Vec<u32>>> {
        each!(self, data => data.unpack(metric, legs, control))
    }
    fn announce(&self, roads: &[u32]) {
        each!(self, data => data.announce(roads))
    }
    fn memory_budget(&self) -> usize {
        each!(self, data => data.memory_budget())
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        each!(self, data => data.set_memory_budget(bytes))
    }
    fn default_budget(&self) -> usize {
        each!(self, data => data.default_budget())
    }
}
