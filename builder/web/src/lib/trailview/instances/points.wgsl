// The producer of stored point instances (points.ts): one lane per instance stands it on the drawn
// surface and emits it.
// #include ../terrain/drawn-mesh.wgsl
// #include emit.wgsl

// As points.ts packs it: the point in whole world units; the model (16 bits), the turn (8 bits) and
// the tint (8 bits); the height in metres (f16) and 16 unused bits.
struct Point {
    at: vec2i,
    model_turn_tint: u32,
    height: u32,
}

@group(0) @binding(8) var<storage, read> points: array<Point>;
// x: the points of the pool.
@group(0) @binding(9) var<uniform> pool: vec4u;

// Points nearer than this stand on drawnHeight, farther ones on the level that draws them: the mesh
// meets it within its morph, below a pixel there.
const MESH_HEIGHT_M: f32 = 400.0;

@compute @workgroup_size(64)
fn stand(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= pool.x) {
        return;
    }
    let p = points[id.x];
    let s = lod_space(vec2f(p.at - surface.anchor));
    var ground: f32;
    if (length(s) < MESH_HEIGHT_M) {
        ground = plain_height(s);
    } else {
        ground = units_height(p.at, u32(ceil(lod(s))));
    }
    if (!known(ground)) {
        return;
    }
    let bits = p.model_turn_tint;
    emit(bits & 0xffffu, render_at(s, ground), unpack2x16float(p.height).x, f32((bits >> 16u) & 255u) / 256.0 * 6.2831853, 1.0,
        0.75 + 0.5 * f32(bits >> 24u) / 255.0, 1.0);
}
