// The corridors' varyings with a piece corner's label, and the mouth's hooks: this file takes the place of
// roads/corridor-in.wgsl in the pieces of a junction with a stop arm (pieces.ts). A pixel in a stop arm's mouth
// (terrain/mouth.wgsl) shades with the arm's look in the arm's frame. Its includes are relative to roads/, the folder
// of the file that it replaces. A slab's pixel near its kerb reads its kerb distance (plan/slab-in.wgsl).
// #include ../terrain/mouth.wgsl
// #include ../plan/line-look.wgsl
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
    // The corner's label in its cell's records.
    @location(7) @interpolate(flat) label: u32,
    @location(8) @interpolate(flat) tail: u32,
}

fn corridor_in(in: CorridorOut) -> CorridorOut {
    let pixel = slab_pixel(in);
    if ((in.word & MAT_BITS) == MAT_GROUND) {
        return in;
    }
    let q = (in.u - vec2f(params.origin - surface.anchor)) * 16.0;
    let mouth = m_mouth(params.block, m_junction(params.block, in.label), q, h_scale(params.block, q));
    if (mouth.line == M_NONE) {
        var out = in;
        out.road.x = slab_across(in.u, in.word, in.road.x, pixel);
        return out;
    }
    let look = line_look(m_line_finish(params.block, mouth.line));
    // A paved arm draws neither its edge nor its edge lines in the mouth, as a junction's look past its frame; a soil
    // arm keeps its tread's own margins.
    let open = PIECE_INSIDE | ((CF_NO_LEFT_LINE | CF_NO_RIGHT_LINE) << FLAGS_SHIFT);
    var out = in;
    out.word = look_word(look.x, look.y, mouth.place.z) | select(open, 0u, soil_look(look.x));
    out.road = mouth.place.xz;
    out.along = mouth.place.y;
    return out;
}

fn corridor_label(out: ptr<function, CorridorOut>, label: u32) {
    (*out).label = label;
}

// A piece's triangle or slot of its tail, near a box (pieces.ts): 1 from its draw's first instance. Its pixel drops
// where it lies in a box (in_box of plan/piece.wgsl).
fn corridor_tail(out: ptr<function, CorridorOut>, tail: u32) {
    (*out).tail = tail;
}

fn corridor_dropped(in: CorridorOut) -> bool {
    return in.tail == 1u && in_box(in.u, in.altitude);
}
