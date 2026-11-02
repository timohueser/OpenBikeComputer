// The skirt of a way's corridor: its rows stand on the stamped ground, over the terrain, which lies under the
// footprint that the way owns. corridor.wgsl draws it; points.wgsl stands a model beside a way on it.
// #include ../terrain/drawn-height.wgsl

// The two rows of a skirt nearest the core's edge, metres past it, as the `near` rows of SKIRT and PATH_SKIRT of
// section.ts. Past them the rows follow the stamped ground out to the daylight line.
const SKIRT_ROWS_M = vec2f(0.35, 1.0);
const PATH_SKIRT_ROWS_M = vec2f(0.3, 0.8);

// The skirt's face at point s of LOD space, `past` metres past the core's edge, which lies along unit `to`: linear
// between its near rows, as the corridor draws it, and before the first one at its height. Past them it is the
// stamped ground, which the rows at shares of the daylight line follow to within their chords.
fn skirt_face(s: vec2f, to: vec2f, past: f32, path: bool, fallback: f32) -> f32 {
    let edge = s + to * past;
    let plane = stamped_height(edge, fallback);
    let rows = select(SKIRT_ROWS_M, PATH_SKIRT_ROWS_M, path);
    if (past > rows.y) {
        return stamped_height(s, plane);
    }
    let t = clamp((past - rows.x) / (rows.y - rows.x), 0.0, 1.0);
    return mix(stamped_height(edge - to * rows.x, plane), stamped_height(edge - to * rows.y, plane), t);
}
