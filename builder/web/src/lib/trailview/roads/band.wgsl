// The band of a way's corridor: its rows stand on the stamped ground, and dive under the drawn mesh where the
// ground is steep. corridor.wgsl draws it; points.wgsl stands a model beside a way on it.
// #include ../terrain/drawn-height.wgsl

// Where the ground climbs or falls BAND_STEEP per metre from the core's outer edge out to a band's row, the
// drawn mesh cannot follow the ground that the band reads, and the two would fight: the band dives
// BAND_DIVE_M under the mesh, which shows alone. A cut's and a fill's cones start at 0.85 (roads.json).
const BAND_DIVE_M: f32 = 1.5;
const BAND_STEEP_LOW: f32 = 0.6;
const BAND_STEEP_HIGH: f32 = 0.8;
// The nearest a row counts from the core's edge, in metres.
const BAND_LEAST_M: f32 = 0.3;

// The stamped ground: one direct page lookup at the level that the terrain draws there, one level coarser
// while that page is on its way, else `fallback`.
fn band_ground(s: vec2f, fallback: f32) -> f32 {
    let level = u32(ceil(lod(s)));
    let fine = bilinear(level, texel_of(s, level));
    if (known(fine)) {
        return fine;
    }
    let coarse = bilinear(level - 1u, texel_of(s, level - 1u));
    return select(fallback, coarse, known(coarse));
}

// How far a row `out` metres past the core's edge dives under `ground`, on a road of plane height `plane`.
fn band_dive(ground: f32, plane: f32, out: f32) -> f32 {
    return BAND_DIVE_M * smoothstep(BAND_STEEP_LOW, BAND_STEEP_HIGH, abs(ground - plane) / max(out, BAND_LEAST_M));
}
