// The corridors' varyings, and the hooks of a stop arm's mouth (plan/mouth-in.wgsl takes this file's place in the
// pieces of a stop arm's junction): no other pixel pays for the mouth.

struct CorridorOut {
    @builtin(position) @invariant clip: vec4f,
    // Units about the anchor.
    @location(0) u: vec2f,
    @location(1) relative: vec3f,
    @location(2) altitude: f32,
    @location(3) @interpolate(flat) word: u32,
    // Metres across and the half-width.
    @location(4) road: vec2f,
    @location(5) along: f32,
    // A piece's normal in LOD-space axes; zero on a corridor.
    @location(6) normal: vec3f,
}

// The pixel as it shades: here as it comes.
fn corridor_in(in: CorridorOut) -> CorridorOut {
    return in;
}

// A piece corner's label in its cell's records: here none is kept.
fn corridor_label(out: ptr<function, CorridorOut>, label: u32) {}
