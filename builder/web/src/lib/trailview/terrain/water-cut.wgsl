// A drawn node keeps the part of a water mesh over its own square, and a triangle with a vertex of
// unknown height draws nothing. This variant discards their fragments; the engine takes
// water-cut-clip.wgsl when the device has clip distances.

struct WaterOut {
    @builtin(position) clip: vec4f,
    @location(0) relative: vec3f,
    @location(1) altitude: f32,
    // A ribbon's side from −1 to 1, its share of the drawn width and its drawn half-width in pixels.
    @location(2) side: f32,
    @location(3) share: f32,
    @location(4) pixels: f32,
    // How steeply a ribbon's bed falls along its flow.
    @location(5) fall: f32,
    // Units inside the node's square from its west, north, east and south edge.
    @location(6) inside: vec4f,
    // 1 where the vertex is kept.
    @location(7) kept: f32,
}

// The fragment shader reads what the vertex shader writes.
alias WaterFragment = WaterOut;

fn mark_cut(out: ptr<function, WaterOut>, inside: vec4f, kept: bool) {
    (*out).inside = inside;
    (*out).kept = select(0.0, 1.0, kept);
}

fn drop_cut(in: WaterFragment) {
    if (any(in.inside < vec4f(0.0)) || in.kept < 0.999) {
        discard;
    }
}
