// The corridors' varyings and the hooks of a piece of the ground plan's mesh: this file takes the place of
// roads/corridor-in.wgsl in the pieces (pieces.ts; plan/mouth-in.wgsl in a stop arm's junction). A slab's pixel near
// its kerb reads its kerb distance (plan/slab-in.wgsl). Its includes are relative to roads/, the folder of the file
// that it replaces.
// #include ../plan/slab-in.wgsl

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
    @location(8) @interpolate(flat) tail: u32,
}

fn corridor_in(in: CorridorOut) -> CorridorOut {
    var out = in;
    out.road.x = slab_across(in.u, in.word, in.road.x, slab_pixel(in));
    return out;
}

// A piece corner's label in its cell's records: here none is kept.
fn corridor_label(out: ptr<function, CorridorOut>, label: u32) {}

// A piece's triangle or slot of its tail, near a box (pieces.ts): 1 from its draw's first instance.
fn corridor_tail(out: ptr<function, CorridorOut>, tail: u32) {
    (*out).tail = tail;
}

fn tail_of(in: CorridorOut) -> u32 {
    return in.tail;
}
