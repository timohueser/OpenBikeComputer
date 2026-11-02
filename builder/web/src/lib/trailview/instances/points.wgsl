// The producer of stored point instances (points.ts): one lane per instance stands it on the drawn
// surface and emits it. A tree takes the snow of the ground under it, and its phase by the terrain of
// BAND_LEVEL, as a forest tree does.
// #include ../ground/look.wgsl
// #include emit.wgsl
// #include ../roads/band.wgsl

// As points.ts packs it: the point in whole world units; the model (16 bits), the turn (8 bits) and
// the tint (8 bits); the height in metres (f16) and the way beside it (16 bits, `packPoints`): 0 for none,
// else the metres to its core's edge in steps of BESIDE_STEP_M, plus 1 (8 bits), the turn toward the edge from
// east in 128ths (7 bits), and whether the model stands on the way's band (1 bit).
struct Point {
    at: vec2i,
    model_turn_tint: u32,
    height: u32,
}

const BESIDE_STEP_M: f32 = 0.02;

@group(0) @binding(8) var<storage, read> points: array<Point>;
// x: the points of the pool.
@group(0) @binding(9) var<uniform> pool: vec4u;

@compute @workgroup_size(64)
fn stand(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= pool.x) {
        return;
    }
    let p = points[id.x];
    let s = lod_space(vec2f(p.at - surface.anchor));
    let stood = stand_ground(s, p.at);
    var ground = stood.x;
    let beside = p.height >> 16u;
    if (beside != 0u && length(s) < MESH_HEIGHT_M) {
        // Beside a way the eye sees the drawn mesh with its snap, which stand_ground leaves out, and on the
        // band also the band's row where that lies over the mesh. Away from the ways the two meshes agree.
        ground = drawn_height(s);
        if ((beside & 0x8000u) != 0u) {
            let out = f32((beside & 255u) - 1u) * BESIDE_STEP_M;
            let turn = f32((beside >> 8u) & 127u) / 128.0 * 6.2831853;
            let band = band_ground(s, ground);
            // The core's edge lies on the way's plane.
            let plane = band_ground(s + out * vec2f(cos(turn), sin(turn)), band);
            ground = max(ground, band - band_dive(band, plane, out));
        }
    }
    if (!known(ground)) {
        return;
    }
    let bits = p.model_turn_tint;
    let model = bits & 0xffffu;
    let height = unpack2x16float(p.height).x;
    let yaw = f32((bits >> 16u) & 255u) / 256.0 * 6.2831853;
    let snow = select(0.0, snow_at(s), models[model].crown >= 0.0);
    let band = units_height(p.at, BAND_LEVEL);
    emit_tree(model, s, ground, height, yaw, 1.0, 0.75 + 0.5 * f32(bits >> 24u) / 255.0, 1.0, 0.0, stood.y, pick_of(yaw, height), snow, vec3f(0.0), select(ground, band, known(band)));
}
