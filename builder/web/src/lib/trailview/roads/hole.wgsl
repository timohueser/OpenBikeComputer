// PROTOTYPE road system (a study; never merges). Where the terrain cuts its hole under the near corridors,
// and where the corridors draw: one rule for both, so a corridor's band always covers the hole. Include
// it with `// #include ../roads/hole.wgsl`.
// #include ../terrain/drawn-mesh.wgsl

// The finest terrain draws from this level on: the corridors draw there and the terrain cuts its hole.
const CORRIDOR_LEVEL: u32 = 17u;
// Metres from a carriageway's edge that the hole reaches, for roads and tracks, and for paths. The bands
// reach 8 m (5 m for a path) past the shoulder or the sidewalk, so they overlap the hole's edge.
const HOLE_ROAD_M: f32 = 6.0;
const HOLE_PATH_M: f32 = 3.0;
// Mask surface codes, as SURFACES of bins.ts: a path, and paving, which a house's walk paints without a corridor.
const HOLE_PATH_CODE: u32 = 6u;
const HOLE_PAVING_CODE: u32 = 8u;
// The corridors reach this share of the mask pool's reach; the hole stops HOLE_MARGIN_M before them.
const CORRIDOR_SHARE: f32 = 0.7;
const HOLE_MARGIN_M: f32 = 40.0;

fn corridor_range() -> f32 {
    return CORRIDOR_SHARE * masks.reach;
}

// Whether the corridors draw at a point of LOD space.
fn corridor_drawn(s: vec2f) -> bool {
    return length(s) < corridor_range() && drawn_level(s) >= CORRIDOR_LEVEL;
}

// Whether the terrain at a point of LOD space lies under a corridor's band: near a road, a track, a path
// or a railway that the mask holds.
fn corridor_hole(s: vec2f) -> bool {
    if (length(s) > corridor_range() - HOLE_MARGIN_M) {
        return false;
    }
    let m = mask_at(s, u32(ceil(lod(s))), true);
    if (m.level == 0u || m.kind.x == 0u || m.kind.x == HOLE_PAVING_CODE) {
        return false;
    }
    return m.edge < select(HOLE_ROAD_M, HOLE_PATH_M, m.kind.x == HOLE_PATH_CODE);
}
