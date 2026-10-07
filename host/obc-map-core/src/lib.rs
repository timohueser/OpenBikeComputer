//! Shared map configuration, integer grid and byte framing. Producer algorithms live above it.

#![warn(clippy::debug_assert_with_mut_call)]

pub mod config;
pub mod grid;
pub mod nav;
pub mod progress;
pub mod semantic;
pub mod serialize;
