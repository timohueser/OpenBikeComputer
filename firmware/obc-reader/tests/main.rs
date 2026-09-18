//! `obc-reader`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

mod common;

#[path = "cases/extremes.rs"]
mod extremes;
#[path = "cases/far_offsets.rs"]
mod far_offsets;
#[path = "cases/format.rs"]
mod format;
#[path = "cases/landmarks.rs"]
mod landmarks;
#[path = "cases/peaks.rs"]
mod peaks;
#[path = "cases/photo.rs"]
mod photo;
#[path = "cases/poi_corridor.rs"]
mod poi_corridor;
#[path = "cases/poi_hours.rs"]
mod poi_hours;
#[path = "cases/poi_query.rs"]
mod poi_query;
#[path = "cases/settlement.rs"]
mod settlement;
