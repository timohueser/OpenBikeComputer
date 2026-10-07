//! Full-map producer and byte-reader integration tests.

#[path = "packer_cases/cell_cut.rs"]
mod cell_cut;
#[path = "packer_cases/contours.rs"]
mod contours;
#[path = "packer_cases/coverage.rs"]
mod coverage;
#[path = "packer_cases/merge_fills.rs"]
mod merge_fills;
#[path = "packer_cases/names.rs"]
mod names;
#[path = "packer_cases/nav_round_trip.rs"]
mod nav_round_trip;
#[path = "packer_cases/pipeline.rs"]
mod pipeline;
#[path = "packer_cases/round_trip.rs"]
mod round_trip;
#[path = "packer_cases/serialize.rs"]
mod serialize;
