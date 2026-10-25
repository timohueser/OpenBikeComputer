// A triangle with a vertex of unknown height draws nothing, nor the terrain in a portal's box. This
// variant discards their fragments; the engine takes unknown-clip.wgsl when the device has clip distances.

struct Varyings {
    @builtin(position) clip: vec4f,
    @location(0) s: vec2f,
    @location(1) relative: vec3f,
    @location(2) known: f32,
    @location(3) h: f32,
    @location(4) @interpolate(flat) level: u32,
    @location(5) hole: f32,
}

// The fragment shader reads what the vertex shader writes.
alias Fragment = Varyings;

fn mark_known(out: ptr<function, Varyings>, known: bool) {
    (*out).known = select(-1e6, 1.0, known);
}

fn mark_hole(out: ptr<function, Varyings>, distance: f32) {
    (*out).hole = distance;
}

fn drop_unknown(in: Fragment) {
    if (in.known < 0.99 || in.hole < 0.0) {
        discard;
    }
}
