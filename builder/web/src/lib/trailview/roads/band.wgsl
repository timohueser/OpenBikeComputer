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
// The rows of a band on the stamped ground, metres past the core's edge, as BAND_M and PATH_BAND_M of section.ts.
const BAND_ROWS_M = vec3f(0.35, 1.0, 2.0);
const PATH_BAND_ROWS_M = vec3f(0.3, 0.8, 0.8);

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

// The height of a band's row `a` metres past the core's edge, which lies at `edge` of LOD space on a plane of
// height `plane`; `out` is the unit way from the edge out.
fn band_row(edge: vec2f, out: vec2f, plane: f32, a: f32) -> f32 {
    let ground = band_ground(edge + out * a, plane);
    return ground - band_dive(ground, plane, a);
}

// The band's face at point s of LOD space, `past` metres past the core's edge, which lies along unit `to`:
// linear between its rows, as the corridor draws it. Before the first row it holds the first row's height.
fn band_face(s: vec2f, to: vec2f, past: f32, path: bool, fallback: f32) -> f32 {
    let edge = s + to * past;
    let plane = band_ground(edge, fallback);
    let rows = select(BAND_ROWS_M, PATH_BAND_ROWS_M, path);
    let i = select(0, 1, past > rows[1]);
    let t = clamp((past - rows[i]) / max(rows[i + 1] - rows[i], 1e-3), 0.0, 1.0);
    return mix(band_row(edge, -to, plane, rows[i]), band_row(edge, -to, plane, rows[i + 1]), t);
}
