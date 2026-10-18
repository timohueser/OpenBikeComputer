//! The functions of the Rust steps of the products, by the names that the child process of a step
//! runs them by. A function that a step list uses and [`FUNCTIONS`] lacks fails its step.

use obc_data::engine::Function;

pub static AREA: Function = Function { name: "obc-osm/area", run: obc_osm::step::area };
pub static OSM: Function = Function { name: "obc-osm/osm", run: obc_osm::step::osm };
pub static LANDMARK_CONTENT: Function =
    Function { name: "obc-pack/landmark-content", run: obc_pack::step::landmark_content };
pub static PEAK_CONTENT: Function = Function { name: "obc-pack/peak-content", run: obc_pack::step::peak_content };
pub static LANDMARKS: Function = Function { name: "obc-pack/landmarks", run: obc_pack::step::landmarks };
pub static PEAKS: Function = Function { name: "obc-pack/peaks", run: obc_pack::step::peaks };
pub static DEVICE_CATALOG: Function = Function { name: "obc-pack/catalog", run: obc_pack::catalog::engine::build };
pub static MAP: Function = Function { name: "obc-bake/map", run: obc_bake::step::map };
pub static TERRAIN: Function = Function { name: "obc-dem/terrain", run: obc_dem::step::terrain };
pub static PLANNER_TERRAIN: Function =
    Function { name: "obc-dem/planner-terrain", run: obc_dem::step::planner_terrain };
pub static ROUTING: Function = Function { name: "planner-router-build/routing", run: planner_router_build::step::step };
pub static SEARCH: Function = Function { name: "obc-search-bake/search", run: obc_search_bake::step::step };
pub static PLANNER_CATALOG: Function =
    Function { name: "obc-data-steps/planner-catalog", run: crate::planner::catalog::pointer };

/// Every function of a Rust step of the products.
pub const FUNCTIONS: &[&Function] = &[
    &AREA,
    &OSM,
    &LANDMARK_CONTENT,
    &PEAK_CONTENT,
    &LANDMARKS,
    &PEAKS,
    &DEVICE_CATALOG,
    &MAP,
    &TERRAIN,
    &PLANNER_TERRAIN,
    &ROUTING,
    &SEARCH,
    &PLANNER_CATALOG,
    &obc_data::engine::PASS,
];
