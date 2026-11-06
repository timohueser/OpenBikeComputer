// The looks of a way's top, as LOOK of section.ts, and a look word's half-width field, as piece.ts: the corridors
// (looks.wgsl) and the pieces of the ground plan (plan/line-look.wgsl) read them.

const LOOK_ASPHALT: u32 = 20u;
const LOOK_MAIN: u32 = 21u;
const LOOK_MOTORWAY: u32 = 22u;
const LOOK_CONCRETE: u32 = 23u;
const LOOK_SETT: u32 = 24u;
const LOOK_PAVERS: u32 = 25u;
const LOOK_GRAVEL: u32 = 26u;
const LOOK_DIRT: u32 = 27u;
const LOOK_TRAIL: u32 = 28u;
const LOOK_BALLAST: u32 = 29u;
const LOOK_STRIPS: u32 = 30u;
const HALF_STEP_M: f32 = 0.025;

// A piece's look word: the look, the drawn code and the half-width.
fn look_word(mat: u32, code: u32, half: f32) -> u32 {
    return mat | (code << 7u) | (min(511u, u32(round(max(half, 0.0) / HALF_STEP_M))) << 19u);
}
