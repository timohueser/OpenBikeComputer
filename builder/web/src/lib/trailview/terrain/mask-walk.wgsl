// The byte of a mask texel that holds what lies past a way's bare surface, unless the texel is a track's: the
// width of the sidewalk on the texel's side, whether the road holds a shoulder, and whether the way is trodden.

const WALK_STEP_M: f32 = 0.1;
const TRODDEN_BIT: u32 = 64u;
const SHOULDER_BIT: u32 = 128u;

fn walk_byte(walk: f32, shoulder: bool, trodden: bool) -> u32 {
    return u32(round(walk / WALK_STEP_M)) | select(0u, SHOULDER_BIT, shoulder) | select(0u, TRODDEN_BIT, trodden);
}

fn walk_metres(byte: u32) -> f32 {
    return f32(byte & (TRODDEN_BIT - 1u)) * WALK_STEP_M;
}

fn is_trodden(byte: u32) -> bool {
    return (byte & TRODDEN_BIT) != 0u;
}

fn holds_shoulder(byte: u32) -> bool {
    return (byte & SHOULDER_BIT) != 0u;
}
