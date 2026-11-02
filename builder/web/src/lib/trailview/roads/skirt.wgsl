// The skirt of a way's corridor: its rows stand on the stamped ground, and dive under the drawn mesh where the
// ground is steep. corridor.wgsl draws it; points.wgsl stands a model beside a way on it.
// #include ../terrain/drawn-height.wgsl

// Where the ground climbs or falls SKIRT_STEEP per metre from the core's outer edge out to a skirt's row, the
// drawn mesh cannot follow the ground that the skirt reads, and the two would fight: the skirt dives
// SKIRT_DIVE_M under the mesh, which shows alone. A cut's and a fill's cones start at 0.85 (roads.json).
const SKIRT_DIVE_M: f32 = 1.5;
const SKIRT_STEEP_LOW: f32 = 0.6;
const SKIRT_STEEP_HIGH: f32 = 0.8;
// The nearest a row counts from the core's edge, in metres.
const SKIRT_LEAST_M: f32 = 0.3;
// The two rows of a skirt nearest the core's edge, metres past it, as the `near` rows of SKIRT and PATH_SKIRT of
// section.ts. Past them the rows follow the stamped ground out to the daylight line.
const SKIRT_ROWS_M = vec2f(0.35, 1.0);
const PATH_SKIRT_ROWS_M = vec2f(0.3, 0.8);

// The stamped ground: one direct page lookup at the level that the terrain draws there, one level coarser
// while that page is on its way, else `fallback`.
fn skirt_ground(s: vec2f, fallback: f32) -> f32 {
    let level = u32(ceil(lod(s)));
    let fine = bilinear(level, texel_of(s, level));
    if (known(fine)) {
        return fine;
    }
    let coarse = bilinear(level - 1u, texel_of(s, level - 1u));
    return select(fallback, coarse, known(coarse));
}

// How far a row `out` metres past the core's edge dives under `ground`, on a road of plane height `plane`.
fn skirt_dive(ground: f32, plane: f32, out: f32) -> f32 {
    return SKIRT_DIVE_M * smoothstep(SKIRT_STEEP_LOW, SKIRT_STEEP_HIGH, abs(ground - plane) / max(out, SKIRT_LEAST_M));
}

// The height of a skirt's row `a` metres past the core's edge, which lies at `edge` of LOD space on a plane of
// height `plane`; `out` is the unit way from the edge out.
fn skirt_row(edge: vec2f, out: vec2f, plane: f32, a: f32) -> f32 {
    let ground = skirt_ground(edge + out * a, plane);
    return ground - skirt_dive(ground, plane, a);
}

// The skirt's face at point s of LOD space, `past` metres past the core's edge, which lies along unit `to`: linear
// between its near rows, as the corridor draws it, and before the first one at its height. Past them it is the
// stamped ground less its dive, which the rows at shares of the daylight line follow to within their chords.
fn skirt_face(s: vec2f, to: vec2f, past: f32, path: bool, fallback: f32) -> f32 {
    let edge = s + to * past;
    let plane = skirt_ground(edge, fallback);
    let rows = select(SKIRT_ROWS_M, PATH_SKIRT_ROWS_M, path);
    if (past > rows.y) {
        return skirt_row(edge, -to, plane, past);
    }
    let t = clamp((past - rows.x) / (rows.y - rows.x), 0.0, 1.0);
    return mix(skirt_row(edge, -to, plane, rows.x), skirt_row(edge, -to, plane, rows.y), t);
}
