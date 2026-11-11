//! A routing library independent of transport, filesystem, map rendering and itinerary policy.
pub mod answer;
pub mod base;
pub mod blocks;
pub mod closures;
pub mod cost;
pub mod data;
#[cfg(not(target_arch = "wasm32"))]
pub mod directory;
pub mod geometry;
pub mod landmarks;
pub mod model;
pub mod package;
mod queue;
pub mod router;
pub mod search;
pub mod shape;
pub mod snap;
pub mod storage;
pub mod table;
#[cfg(not(target_arch = "wasm32"))]
pub use directory::open;
pub use router::{Control, Request, Route, Router};

/// A request that exceeds a fixed limit, and fails again on an idle service with the same data,
/// gives `Limit`. A request that fails for memory that other requests hold, or for an allocation
/// that fails, gives `Memory`: a retry can succeed.
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
    #[error("No route connects the selected points")]
    NoPath,
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
