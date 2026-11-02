// The word per mesh vertex of a mask's page that the snap pass of stamp.wgsl writes and drawn-mesh.wgsl reads: the
// vertex's move onto the nearest road edge in whole units, x and y as 10-bit signed fields (a move stays within 3.75
// texels, 240 units at the coarsest level with masks), and its drop under the corridor of the way that owns its
// ground, in steps of DROP_STEP_M, in the 6 bits above each.

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
