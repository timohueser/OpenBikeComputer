enable clip_distances;

// A triangle with a vertex of unknown height draws nothing, nor the terrain in a portal's box. The GPU
// clips them where their distances interpolate below 0, the rule of unknown.wgsl. No fragment shader discards: a discard turns off
// the hidden-surface removal of tile-based GPUs such as Apple's.

struct Varyings {
    @builtin(position) clip: vec4f,
    @builtin(clip_distances) known: array<f32, 2>,
    @location(0) s: vec2f,
    @location(1) relative: vec3f,
    @location(3) h: f32,
    @location(4) @interpolate(flat) level: u32,
}

// A fragment shader cannot read a clip distance.
struct Fragment {
    @builtin(position) clip: vec4f,
    @location(0) s: vec2f,
    @location(1) relative: vec3f,
    @location(3) h: f32,
    @location(4) @interpolate(flat) level: u32,
}

fn mark_known(out: ptr<function, Varyings>, known: bool) {
    (*out).known[0] = select(-1e6, 1.0, known) - 0.99;
}

fn mark_hole(out: ptr<function, Varyings>, distance: f32) {
    (*out).known[1] = distance;
}

fn drop_unknown(in: Fragment) {}
