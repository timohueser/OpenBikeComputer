// The word per mesh vertex of a mask's page that the snap pass of stamp.wgsl writes and drawn-mesh.wgsl reads: the
// vertex's move onto the nearest road edge in whole units, x and y as 10-bit signed fields (a move stays within 3.75
// texels, 240 units at the coarsest level with masks), and its drop under the corridor of the way that owns its
// ground, in steps of DROP_STEP_M, in the 6 bits above each. Beside it the cover word, per texel of the vertex's cell
// (the 4 × 4 texels from its corner): whether a corridor's face covers the texel's centre in plan, and whether that face
// is a way's surface, as roads/cover.wgsl draws them.

const DROP_STEP_M: f32 = 0.01;
const DROP_STEPS: u32 = 4095u;

fn snap_word(moved: vec2i, drop: f32) -> u32 {
    let q = u32(min(round(drop / DROP_STEP_M), f32(DROP_STEPS)));
    return (bitcast<u32>(moved.x) & 0x3ffu) | ((q & 63u) << 10u) | ((bitcast<u32>(moved.y) & 0x3ffu) << 16u) | ((q >> 6u) << 26u);
}

fn snap_moved(word: u32) -> vec2i {
    return vec2i(bitcast<i32>(word << 22u) >> 22u, bitcast<i32>(word << 6u) >> 22u);
}

fn snap_drop(word: u32) -> f32 {
    return f32(((word >> 10u) & 63u) | ((word >> 26u) << 6u)) * DROP_STEP_M;
}

// The bit of a texel in the cover word of its cell, or of its way's surface with `surface`.
fn cover_bit(texel: vec2i, surface: bool) -> u32 {
    return 1u << (u32((texel.y & 3) * 4 + (texel.x & 3)) + select(0u, 16u, surface));
}
