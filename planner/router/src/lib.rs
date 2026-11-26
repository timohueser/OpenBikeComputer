//! A routing library independent of transport, filesystem, map rendering and itinerary policy.
pub mod answer;
pub mod cells;
pub mod closures;
#[cfg(not(target_arch = "wasm32"))]
pub mod compare;
pub mod cost;
pub mod geometry;
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
pub mod model;
pub mod overlay;
pub mod region;
pub mod router;
pub mod shape;
mod shapes;
pub mod snap;
pub use router::{Control, Request, Route, Router};

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// Opens the install of format 10 in `path`, which `install.bin` marks complete; with `silent`, on
/// a host whose `announce` does nothing, which only a measurement uses.
#[cfg(not(target_arch = "wasm32"))]
pub fn open(path: &Path, silent: bool) -> Result<region::Region<host::Maps>> {
    if !path.join("directory.bin").is_file() || !path.join(install::MARKER).is_file() {
        return Err(Error::MissingRegion(format!("The routing install in {} is not complete", path.display())));
    }
    let read = |name: &str| {
        let path = path.join(name);
        std::fs::read(&path).map_err(|e| Error::InvalidData(format!("{}: {e}", path.display())))
    };
    let (directory, info, marker) = (read("directory.bin")?, read("package.json")?, read(install::MARKER)?);
    let maps = host::Maps::new(path, &directory, &marker)?;
    region::Region::open(if silent { maps.silent() } else { maps }, directory, info, &marker)
}

/// A request that exceeds a fixed limit, such as the search limit of the worker, gives `Limit`: it
/// fails again with the same data. A request for which the system refuses memory, a map or a file
/// handle gives `Memory`: a retry can succeed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Routing data is missing: {0}")]
    MissingRegion(String),
    #[error("Invalid routing data: {0}")]
    InvalidData(String),
    #[error("No accessible road near point {0}")]
    NoSnap(usize),
    /// The index of the leg in the request: the leg from point `n` to point `n + 1`.
    #[error("No route connects the selected points")]
    NoPath(usize),
    #[error("Routing was cancelled")]
    Cancelled,
    #[error("Routing resource limit reached")]
    Limit,
    #[error("A routing worker has no memory for this request now")]
    Memory,
    #[error("The line exceeds the shaping limits")]
    LineTooLong,
    #[error("No plan follows the line on roads")]
    NotReproducible,
}

pub type Result<T> = std::result::Result<T, Error>;
