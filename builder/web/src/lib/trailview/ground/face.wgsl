// The ground look's detail on a steep face, in the face's own plane. A plan texture stretches up a face by 1 / n.z, so
// a steep face takes its detail from side projections as well: three fixed planes, so the detail stays in place as
// the eye moves. look.wgsl includes it; the terrain and a corridor's skirt read it with the same normal, so one point
// keeps one colour.
// #include look.wgsl

// A projection draws alone while it stretches its texture at most 1.25 times: the plan up to a slope of 37° (n.z
// 0.8), a side for a face up to 37° off its axis. Between, over the 16° about 45°, two blend by a smooth step.
const FACE_ALONE: f32 = 0.8;
const FACE_SHARED: f32 = 0.6;

struct Facing {
    // The shares of the plan (x, y), the side (x, z) and the side (y, z), which sum to 1. z is the altitude.
    shares: vec3f,
    // The point: metres east and north in the frame, and its altitude.
    point: vec3f,
}

// How the ground at `point` faces, by its normal `n` in render space.
fn facing_of(n: vec3f, point: vec3f) -> Facing {
    let side = 1.0 - smoothstep(FACE_SHARED, FACE_ALONE, n.z);
    if (side <= 0.0) {
        return Facing(vec3f(1.0, 0.0, 0.0), point);
    }
    let across_x = smoothstep(FACE_SHARED, FACE_ALONE, abs(n.x) / max(length(n.xy), 1e-6));
    return Facing(vec3f(1.0 - side, side * (1.0 - across_x), side * across_x), point);
}

// Projection i of a point or a step: the plan, the side (x, z) or the side (y, z).
fn projected(v: vec3f, i: u32) -> vec2f {
    return select(select(v.yz, v.xz, i == 1u), v.xy, i == 0u);
}

// Value noise of cells `size` metres across on the face, moved by `offset` cells. Where projections blend, their sum
// keeps the contrast of one noise, so a threshold on it draws crisp shapes, as on flat ground.
fn face_noise(f: Facing, size: f32, offset: f32) -> f32 {
    if (f.shares.x >= 1.0) {
        return noise(f.point.xy / size + offset);
    }
    var sum = 0.0;
    for (var i = 0u; i < 3u; i++) {
        if (f.shares[i] > 0.0) {
            sum += f.shares[i] * (noise(projected(f.point, i) / size + offset) - 0.5);
        }
    }
    return 0.5 + sum / length(f.shares);
}

// The span of a pixel whose screen steps are `a` and `b` metres, in the face's projections: the larger and the
// smaller, as the footprint and the lateral span are in the plan.
fn face_spans(f: Facing, a: vec3f, b: vec3f) -> vec2f {
    var spans = vec2f(0.0);
    for (var i = 0u; i < 3u; i++) {
        if (f.shares[i] > 0.0) {
            let pa = length(projected(a, i));
            let pb = length(projected(b, i));
            spans += f.shares[i] * vec2f(max(pa, pb), min(pa, pb));
        }
    }
    return spans;
}
