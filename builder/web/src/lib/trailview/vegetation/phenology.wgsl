// The trees' year, as phenology.ts: four turns, leaf-out, full green, autumn colour and leaf fall,
// which each tree passes on a day of its own by its hash.

// Per turn: its day of the year at REFERENCE_M, the days over which a stand passes it, and the days
// that it comes later per metre higher (TURNS of phenology.ts).
const TURN_LEAF_OUT: vec3f = vec3f(110.0, 30.0, 0.03);
const TURN_GREEN: vec3f = vec3f(155.0, 30.0, 0.03);
const TURN_COLOUR: vec3f = vec3f(272.0, 40.0, -0.015);
const TURN_FALL: vec3f = vec3f(308.0, 30.0, -0.015);
const REFERENCE_M: f32 = 500.0;
const TREELINE_M: f32 = 2500.0;
// The phases between the turns: winter (bare), spring, summer, autumn.
const PHASE_WINTER: u32 = 0u;
const PHASE_SUMMER: u32 = 2u;

// The share of the trees at an elevation that have passed a turn on a day.
fn turned(turn: vec3f, day: f32, elevation: f32) -> f32 {
    return clamp((day - turn.x - turn.z * (clamp(elevation, 0.0, TREELINE_M) - REFERENCE_M)) / turn.y + 0.5, 0.0, 1.0);
}

// The shares of the trees past each turn, in their order.
fn turns_passed(day: f32, elevation: f32) -> vec4f {
    return vec4f(turned(TURN_LEAF_OUT, day, elevation), turned(TURN_GREEN, day, elevation), turned(TURN_COLOUR, day, elevation), turned(TURN_FALL, day, elevation));
}

// The phase of a tree whose hash is `u` (0 to 1): it has passed each turn whose share exceeds `u`.
fn phase_of(u: f32, day: f32, elevation: f32) -> u32 {
    let passed = vec4u(vec4f(u) < turns_passed(day, elevation));
    return (passed.x + passed.y + passed.z + passed.w) & 3u;
}

// The share of the trees in each phase: winter, spring, summer, autumn.
fn phase_shares(day: f32, elevation: f32) -> vec4f {
    let s = turns_passed(day, elevation);
    return vec4f(1.0 - s.x + s.w, s.x - s.y, s.y - s.z, s.z - s.w);
}
