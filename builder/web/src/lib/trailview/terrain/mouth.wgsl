// A stop arm's mouth: in a junction's face, within the arm's half-width and MOUTH_ROUND_M of its centre line and off
// its hosts' carriageways, the face wears the arm's own look. The pieces (plan/piece.wgsl) and the paint (stamp.wgsl)
// read the one rule.
// #include records.wgsl

// The mouth's rounds, as the junction's corner radius of a stop arm.
const MOUTH_ROUND_M: f32 = 1.0;
// Over this width at the mouth's sides and at the hosts' edges the arm's look fades into the junction's.
const MOUTH_BLEND_M: f32 = 0.5;
const MOUTH_JUNCTION: u32 = 2u;

// The stop arm line (M_NONE for none), the share of its look, and the metres across (signed by side), along and the
// half-width of its frame.
struct Mouth {
    line: u32,
    share: f32,
    across: f32,
    along: f32,
    half: f32,
}

// Whether label `id` is a junction with a stop arm.
fn m_has_mouth(block: u32, id: u32) -> bool {
    if (id == M_NONE) {
        return false;
    }
    let l = m_label(block, id);
    var stop = false;
    for (var w = 8u; w < 12u; w++) {
        stop = stop || (mw(l + w) != M_NONE && (mw(l + w) & STOP_ARM) != 0u);
    }
    return ((mw(l) >> 8u) & 0xffu) == MOUTH_JUNCTION && stop;
}

// The mouth of junction label `id` at q, from the `n` centre-line segments of its bin at `segs`: of its stop arms the
// one whose side lies nearest, and the share that is 1 inside the mouth and 0 past its sides or on a host's
// carriageway.
fn m_mouth(block: u32, id: u32, q: vec2f, segs: u32, n: u32, mpu: f32) -> Mouth {
    var out = Mouth(M_NONE, 0.0, 0.0, 0.0, 0.0);
    if (!m_has_mouth(block, id)) {
        return out;
    }
    let l = m_label(block, id);
    let samples = block + mw(block + 7u);
    let lines = block + mw(block + 6u);
    // The metres past the mouth's side and past the nearest host's edge.
    var side = 1e9;
    var host = 1e9;
    for (var k = 0u; k < n; k++) {
        let s = mw(segs + k);
        let line = s & 0xffffu;
        var hosted = false;
        for (var w = 4u; w < 8u; w++) {
            hosted = hosted || mw(l + w) == line;
        }
        var stop = false;
        for (var w = 8u; w < 12u; w++) {
            stop = stop || ((mw(l + w) & STOP_ARM) != 0u && (mw(l + w) & ~STOP_ARM) == line && mw(l + w) != M_NONE);
        }
        if (!hosted && !stop) {
            continue;
        }
        let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
        let wb = wa + M_SAMPLE;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let d = length(q - a - t * u) * mpu;
        let half = mix(mf(wa + 2u), mf(wb + 2u), t) * mpu;
        if (hosted) {
            host = min(host, d - half);
        } else if (d - half - MOUTH_ROUND_M < side) {
            side = d - half - MOUTH_ROUND_M;
            out.line = line;
            out.across = sign(m_cross(u, q - a)) * d;
            out.along = mix(mf(wa + 3u), mf(wb + 3u), t);
            out.half = half;
        }
    }
    if (out.line != M_NONE) {
        let b = 0.5 * MOUTH_BLEND_M;
        out.share = (1.0 - smoothstep(-b, b, side)) * smoothstep(-b, b, host);
    }
    return out;
}
