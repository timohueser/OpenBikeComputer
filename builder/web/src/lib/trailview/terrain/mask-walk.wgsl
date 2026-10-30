// The byte of a mask texel that holds what lies past a road's edge, unless the texel is a track's: the
// width of the sidewalk on the texel's side, and whether the road holds a shoulder.

const WALK_STEP_M: f32 = 0.1;
const SHOULDER_BIT: u32 = 128u;

fn walk_byte(walk: f32, shoulder: bool) -> u32 {
    return u32(round(walk / WALK_STEP_M)) | select(0u, SHOULDER_BIT, shoulder);
}

fn walk_metres(byte: u32) -> f32 {
    return f32(byte & (SHOULDER_BIT - 1u)) * WALK_STEP_M;
}

fn holds_shoulder(byte: u32) -> bool {
    return (byte & SHOULDER_BIT) != 0u;
}
