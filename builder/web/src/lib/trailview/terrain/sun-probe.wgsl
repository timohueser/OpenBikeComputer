// The share of the sun at points of LOD space, read back to the CPU by the check. It has its own
// module, so the probe and the check that every start compiles stay without the ground look.
// #include ../ground/look.wgsl

@group(0) @binding(0) var<storage, read> points: array<vec2f>;
@group(0) @binding(1) var<storage, read_write> probed: array<f32>;

@compute @workgroup_size(64)
fn sun_probe(@builtin(global_invocation_id) id: vec3u) {
    if (id.x < arrayLength(&points)) {
        probed[id.x] = sun_at(points[id.x]);
    }
}
