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

// Frame of terrain.wgsl up to the eye's height, whose camera `clamp_eye` lifts: a change to Frame's start changes it too.
struct EyeFrame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
}

@group(0) @binding(2) var<storage, read_write> eye_frame: EyeFrame;

// The least height of the drawn eye above the drawn surface under it.
const EYE_CLEARANCE_M: f32 = 0.5;

// Lifts this frame's camera to EYE_CLEARANCE_M above the drawn surface at the eye, the origin of LOD
// space. The CPU eye stands on a probe one to three frames late, and a page, stamp or mask that lands in
// the meantime changes the drawn surface at once. Only the view projection moves: render space stays
// about the CPU eye, as the shadow maps, the look and the resolve have it.
@compute @workgroup_size(1)
fn clamp_eye() {
    let ground = drawn_height(vec2f(0.0));
    if (known(ground)) {
        let lift = max(0.0, ground + EYE_CLEARANCE_M - eye_frame.eye);
        eye_frame.view_proj[3] -= lift * eye_frame.view_proj[2];
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
