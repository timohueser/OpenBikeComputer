enable clip_distances;

// A drawn node keeps the part of a water mesh over its own square, and a triangle with a vertex of
// unknown height draws nothing. The GPU clips the rest where its distances interpolate below 0, the
// rule of water-cut.wgsl. No fragment shader discards: a discard turns off the hidden-surface removal
// of tile-based GPUs such as Apple's.

struct WaterOut {
    @builtin(position) clip: vec4f,
    @builtin(clip_distances) cut: array<f32, 5>,
    @location(0) relative: vec3f,
    @location(1) altitude: f32,
    @location(2) side: f32,
    @location(3) share: f32,
    @location(4) pixels: f32,
    @location(5) fall: f32,
}

// A fragment shader cannot read a clip distance.
struct WaterFragment {
    @builtin(position) clip: vec4f,
    @location(0) relative: vec3f,
    @location(1) altitude: f32,
    @location(2) side: f32,
    @location(3) share: f32,
    @location(4) pixels: f32,
    @location(5) fall: f32,
}

fn mark_cut(out: ptr<function, WaterOut>, inside: vec4f, kept: bool) {
    (*out).cut = array(inside.x, inside.y, inside.z, inside.w, select(0.0, 1.0, kept) - 0.999);
}

fn drop_cut(in: WaterFragment) {}
