//! `obc-pack`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

#[path = "cases/cell_cut.rs"]
mod cell_cut;
#[path = "cases/contours.rs"]
mod contours;
#[path = "cases/coverage.rs"]
mod coverage;
#[path = "cases/landmark_map.rs"]
mod landmark_map;
#[path = "cases/merge_fills.rs"]
mod merge_fills;
#[path = "cases/names.rs"]
mod names;
#[path = "cases/nav_round_trip.rs"]
mod nav_round_trip;
#[path = "cases/pipeline.rs"]
mod pipeline;
#[path = "cases/round_trip.rs"]
mod round_trip;
#[path = "cases/serialize.rs"]
mod serialize;
