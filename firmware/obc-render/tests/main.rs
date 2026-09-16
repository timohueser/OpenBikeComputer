//! `obc-render`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

mod common;

#[path = "cases/arrows.rs"]
mod arrows;
#[path = "cases/errors.rs"]
mod errors;
#[path = "cases/fill_edges.rs"]
mod fill_edges;
#[path = "cases/marker.rs"]
mod marker;
#[path = "cases/priority.rs"]
mod priority;
#[path = "cases/projection.rs"]
mod projection;
#[path = "cases/static_scene.rs"]
mod static_scene;
#[path = "cases/stroke.rs"]
mod stroke;
#[path = "cases/terrain_layer.rs"]
mod terrain_layer;
#[path = "cases/text.rs"]
mod text;
