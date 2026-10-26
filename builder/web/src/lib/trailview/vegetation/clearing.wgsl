// The clearing that moves with the eye: no crown of a tree or a shrub fills the view from close
// by. A wide spruce steps back farther than a slim tree, and a crown high above the eye may stand
// over it.
// #include ../terrain/view.wgsl

// Each crown keeps EYE_CLEAR_M plus up to EYE_CLEAR_SPREAD_M by its hash from the eye, so trees leave
// one by one and a still view draws none in part.
const EYE_CLEAR_M: f32 = 3.0;
const EYE_CLEAR_SPREAD_M: f32 = 1.0;

// Metres from the eye to a crown: an upright cylinder of `reach` metres about the stem at `s`, from
// `low` to `high` metres above sea level.
fn crown_distance(s: vec2f, low: f32, high: f32, reach: f32) -> f32 {
    return length(vec2f(max(length(s) - reach, 0.0), max(max(low - frame.eye, frame.eye - high), 0.0)));
}
