// The corridors' varyings and the hooks of a piece of the ground plan's mesh: this file takes the place of
// roads/corridor-in.wgsl in the pieces (pieces.ts; plan/mouth-in.wgsl in a stop arm's junction). A slab's pixel near
// its kerb reads its kerb distance (plan/slab-in.wgsl). Its includes are relative to roads/, the folder of the file
// that it replaces.
// #include ../plan/slab-in.wgsl
// #include ../terrain/portal-box.wgsl

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


// PROTOTYPE parts: the terrain's boxes (holes of terrain.wgsl); a piece's pixel of ground in a box draws nothing.
@group(0) @binding(9) var<storage, read> part_holes: array<vec4f>;

fn part_hole(in: CorridorOut) {
    for (var i = 0u; i < u32(part_holes[0].x); i++) {
        let b = part_holes[2u + 3u * i];
        let q = box_point(part_holes[1u + 3u * i], part_holes[3u + 3u * i].x, in.u, in.altitude);
        if (max(max(-q.x, q.x - b.x), q.y - b.y) < 0.0 && q.z > b.w + 0.2 && q.z < b.z) {
            discard;
        }
    }
}

fn corridor_in(in: CorridorOut) -> CorridorOut {
    part_hole(in);
    var out = in;
    out.road.x = slab_across(in.u, in.word, in.road.x, slab_pixel(in));
    return out;
}

// A piece corner's label in its cell's records: here none is kept.
fn corridor_label(out: ptr<function, CorridorOut>, label: u32) {}
