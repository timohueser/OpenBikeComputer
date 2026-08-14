//! Reader error and decode-status vocabulary.

use crate::Error;
use obc_formats::io::Error as IoError;

/// Which caller-owned feature scratch bound rejected a complete encoded feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    Points,
    Rings,
}

/// Why a feature was consumed but not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureDecodeError {
    Capacity(CapacityError),
    Malformed,
}

/// A single-feature refetch failure, retaining decode/capacity vs. source/cache identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureReadError {
    Decode(FeatureDecodeError),
    Read(MapReadError),
}

/// A cache access failed without panicking through the safe reader API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError {
    Busy,
}

/// Failures while streaming a map index or geometry chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapReadError {
    Source(IoError),
    Cache(CacheError),
    Malformed,
}

impl From<MapReadError> for Error {
    fn from(error: MapReadError) -> Self {
        match error {
            MapReadError::Source(error) => Error::Source(error),
            MapReadError::Cache(CacheError::Busy) => Error::CacheBusy,
            MapReadError::Malformed => Error::BadOffset,
        }
    }
}

/// Outcome of a feature-chunk walk. Failed features are consumed whole and never visited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStatus {
    pub complete: u32,
    pub capacity_dropped: u32,
    pub malformed: u32,
}

impl DecodeStatus {
    #[inline]
    pub(super) fn dropped(&mut self, error: FeatureDecodeError) {
        match error {
            FeatureDecodeError::Capacity(_) => self.capacity_dropped = self.capacity_dropped.saturating_add(1),
            FeatureDecodeError::Malformed => self.malformed = self.malformed.saturating_add(1),
        }
    }
}
