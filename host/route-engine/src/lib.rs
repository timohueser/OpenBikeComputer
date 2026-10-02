//! A routing library independent of transport, filesystem, map rendering and itinerary policy.
mod alternatives;
pub mod answer;
pub mod base;
pub mod blocks;
pub mod cost;
pub mod data;
#[cfg(not(target_arch = "wasm32"))]
pub mod directory;
pub mod endpoints;
pub mod geometry;
pub mod landmarks;
pub mod model;
pub mod osm;
pub mod package;
mod queue;
pub mod router;
pub mod search;
pub mod snap;
pub mod storage;
pub mod table;
pub use router::{Control, Request, Route, Router};

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
}

pub type Result<T> = std::result::Result<T, Error>;
