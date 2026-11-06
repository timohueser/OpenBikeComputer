// A stop arm's mouth. In a junction's face with an unpaved arm that stops at the host (STOP_ARM), the route is the
// smooth union of the strokes of the other arms; off the route, a point within the stop arm's corner radius of its
// edge, or nearer to its stroke than to the route's, wears the stop arm's look; the rest keeps the junction's. The
// pieces (plan/piece.wgsl) and the paint (stamp.wgsl) read the one rule.
// #include records.wgsl

// The route's two nearest arms meet in rounds of about this radius, the least corner radius of the classes.
const ROUTE_ROUND_M: f32 = 1.0;
const MOUTH_JUNCTION: u32 = 2u;

// The stop arm line nearest q (M_NONE for none), the share of the junction's look (the rest is the arm's), and the
// metres across (signed by side), along and the half-width of the arm's frame.
struct Mouth {
    line: u32,
    keep: f32,
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

// The polynomial smooth minimum of a and b over k.
fn m_smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

// The mouth of junction label `id` at q, from the `n` centre-line segments of its bin at `segs`. Past the route's
// edge, and about the other borders of the stop arm's look, the two looks blend over `blend` metres: a pixel's width
// gives an edge as sharp as a face's.
fn m_mouth(block: u32, id: u32, q: vec2f, segs: u32, n: u32, mpu: f32, blend: f32) -> Mouth {
    var out = Mouth(M_NONE, 1.0, 0.0, 0.0, 0.0);
    if (!m_has_mouth(block, id)) {
        return out;
    }
    let l = m_label(block, id);
    let hosts = vec4u(mw(l + 4u), mw(l + 5u), mw(l + 6u), mw(l + 7u));
    let others = vec4u(mw(l + 8u), mw(l + 9u), mw(l + 10u), mw(l + 11u));
    // The metres past the nearest route arm's edge and past the next route arm's (a line of its own), and past the
    // nearest stop arm's edge.
    var kept = 1e9;
    var kept_line = M_NONE;
    var next = 1e9;
    var arm = 1e9;
    var reach = 0.0;
    let samples = block + mw(block + 7u);
    let lines = block + mw(block + 6u);
    for (var k = 0u; k < n; k++) {
        let s = mw(segs + k);
        let line = s & 0xffffu;
        // A stop arm's word, or 0 for a route arm, or M_NONE for none.
        var word = select(M_NONE, 0u, any(hosts == vec4u(line)));
        for (var j = 0u; j < 4u; j++) {
            if (m_arm_line(others[j]) == line) {
                word = select(0u, others[j], (others[j] & STOP_ARM) != 0u);
            }
        }
        if (word == M_NONE) {
            continue;
        }
        let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
        let wb = wa + M_SAMPLE;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let d = length(q - a - t * u) * mpu;
        let half = mix(mf(wa + 2u), mf(wb + 2u), t) * mpu;
        let r = f32((word >> 16u) & 0x7fffu) / 100.0;
        if (r == 0.0) {
            if (line == kept_line) {
                kept = min(kept, d - half);
            } else if (d - half < kept) {
                next = kept;
                kept = d - half;
                kept_line = line;
            } else {
                next = min(next, d - half);
            }
        } else if (d - half < arm) {
            arm = d - half;
            reach = r;
            out.line = line;
            out.across = sign(m_cross(u, q - a)) * d;
            out.along = mix(mf(wa + 3u), mf(wb + 3u), t);
            out.half = half;
        }
    }
    kept = m_smooth_min(kept, next, ROUTE_ROUND_M);
    if (out.line == M_NONE) {
        return out;
    }
    let w = max(blend, 1e-3);
    let b = 0.5 * w;
    let near = max(1.0 - smoothstep(reach - b, reach + b, arm), 1.0 - smoothstep(-b, b, arm - kept));
    out.keep = 1.0 - smoothstep(0.0, w, kept) * near;
    return out;
}
