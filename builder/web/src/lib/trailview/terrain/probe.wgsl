// The probe: drawnHeight at points of LOD space, read back to the CPU, such as the ground under the
// eye. `check_vertex` evaluates the same points in a vertex shader, as an object instance does.
// #include drawn-mesh.wgsl

@group(0) @binding(0) var<storage, read> points: array<vec2f>;
@group(0) @binding(1) var<storage, read_write> probed: array<f32>;

@compute @workgroup_size(64)
fn probe(@builtin(global_invocation_id) id: vec3u) {
    if (id.x < arrayLength(&points)) {
        probed[id.x] = drawn_height(points[id.x]);
    }
}

struct Check {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) height: f32,
}

// Point i lands on pixel i of a row as wide as the points.
@vertex
fn check_vertex(@builtin(vertex_index) i: u32) -> Check {
    let n = f32(arrayLength(&points));
    return Check(vec4f((f32(i) + 0.5) / n * 2.0 - 1.0, 0.0, 0.0, 1.0), drawn_height(points[i]));
}

@fragment
fn check_fragment(in: Check) -> @location(0) vec4f {
    return vec4f(in.height, 0.0, 0.0, 1.0);
}
