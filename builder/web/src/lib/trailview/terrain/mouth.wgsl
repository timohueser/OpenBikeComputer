// A stop arm's mouth. In a junction's face with an unpaved arm that stops at the host (M_ARM_STOP), the other arms
// (the route) divide the plane about the node into sectors. In a sector with a stop arm, a point keeps the junction's
// look on a route arm's stroke or in the mitre where the two route arms that bound the sector meet, and wears the
// nearest stop arm's look elsewhere. The pieces (plan/mouth-in.wgsl) and the paint (model.wgsl) read the one rule.
// #include records.wgsl

// The face's edge lies up to this outside a route arm's stroke (snap and chords): a point within it keeps the route.
const ROUTE_EPS_M: f32 = 0.02;
// The mitre of two route arms reaches at most this many of their greater half-width from the node.
const MITRE_LIMIT: f32 = 4.0;
const M_TAU: f32 = 6.2831853;

// The first sample word of arm k's line.
fn m_arm_samples(block: u32, jr: u32, k: u32) -> u32 {
    return block + mw(block + 7u) + M_SAMPLE * mw(block + mw(block + 6u) + M_LINE * (mw(m_arm(jr, k)) & 0xffffu) + 2u);
}

// The direction of arm k at the node.
fn m_arm_dir(jr: u32, k: u32) -> vec2f {
    return unpack2x16snorm(mw(m_arm(jr, k) + 2u));
}

fn m_wrap(a: f32) -> f32 {
    return a - M_TAU * floor(a / M_TAU);
}

// The signed distance in units from q to arm k's stroke: its centre line from the node to its last sample less the
// half-width, with a flat end at the node. On its side `side` (+1 left, -1 right of its direction, 0 none) it is wider
// by `widen` at the node, less so along it, and as wide as the line at the last sample.
fn m_stroke(block: u32, jr: u32, k: u32, q: vec2f, side: f32, widen: f32) -> f32 {
    let range = mw(m_arm(jr, k) + 1u);
    let node = range & 0xffffu;
    let last = range >> 16u;
    let segments = max(node, last) - min(node, last);
    let samples = m_arm_samples(block, jr, k);
    var total = 0.0;
    for (var m = 0u; m < segments; m++) {
        let i = select(node - m, node + m, last > node);
        let j = select(i - 1u, i + 1u, last > node);
        total += length(vec2f(m_vertex(samples + M_SAMPLE * j) - m_vertex(samples + M_SAMPLE * i)));
    }
    var d = M_FAR;
    var along = 0.0;
    for (var m = 0u; m < segments; m++) {
        let i = select(node - m, node + m, last > node);
        let wa = samples + M_SAMPLE * i;
        let wb = samples + M_SAMPLE * select(i - 1u, i + 1u, last > node);
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let len = length(u);
        let t = dot(q - a, u) / max(dot(u, u), 1e-9);
        let c = clamp(t, 0.0, 1.0);
        let wider = select(0.0, widen * (1.0 - (along + c * len) / max(total, 1e-6)), side * m_cross(u, q - a) > 0.0);
        if (m == 0u && t < 0.0) {
            d = min(d, max(abs(m_cross(u, q - a)) / max(len, 1e-6) - mf(wa + 2u) - wider, -t * len));
        } else {
            d = min(d, length(q - a - c * u) - mix(mf(wa + 2u), mf(wb + 2u), c) - wider);
        }
        along += len;
    }
    return d;
}

// The stop arm whose look q wears in the face of junction record jr (M_NONE for none), with its frame; line M_NONE
// where q keeps the junction's look. An arm with no segment bounds no sector.
fn m_mouth(block: u32, jr: u32, q: vec2f, mpu: f32) -> ArmFrame {
    let keep = ArmFrame(M_NONE, vec3f(M_FAR, 0.0, 0.0));
    if (jr == M_NONE) {
        return keep;
    }
    let n = mw(jr + 2u) & 0xffu;
    let v = q - vec2f(m_vertex(jr));
    let aq = atan2(v.y, v.x);
    // The route arms that bound q's sector: the nearest behind q and ahead of it, counter-clockwise.
    var a = M_NONE;
    var b = M_NONE;
    var behind = M_TAU;
    var ahead = M_TAU;
    var any_stop = false;
    for (var k = 0u; k < n; k++) {
        let range = mw(m_arm(jr, k) + 1u);
        if ((range & 0xffffu) == (range >> 16u)) {
            continue;
        }
        if ((mw(m_arm(jr, k)) & M_ARM_STOP) != 0u) {
            any_stop = true;
            continue;
        }
        let u = m_arm_dir(jr, k);
        let t = atan2(u.y, u.x);
        if (m_wrap(aq - t) < behind) {
            behind = m_wrap(aq - t);
            a = k;
        }
        if (m_wrap(t - aq) < ahead) {
            ahead = m_wrap(t - aq);
            b = k;
        }
    }
    if (!any_stop || a == M_NONE) {
        return keep;
    }
    // A sector without a stop arm keeps the junction's look: its fillets shape it.
    let ua = m_arm_dir(jr, a);
    let ub = m_arm_dir(jr, b);
    let ta = atan2(ua.y, ua.x);
    let ha = mf(m_arm_samples(block, jr, a) + M_SAMPLE * (mw(m_arm(jr, a) + 1u) & 0xffffu) + 2u);
    let hb = mf(m_arm_samples(block, jr, b) + M_SAMPLE * (mw(m_arm(jr, b) + 1u) & 0xffffu) + 2u);
    // The facing edges at the node: where one arm's edge corner lies past the other's edge line (two arms of
    // different widths that run on straight), the other widens to it at the node, back to its own width at its seam.
    let na = vec2f(-ua.y, ua.x);
    let nb = vec2f(ub.y, -ub.x);
    let two = select(0.0, 1.0, a != b);
    let wa = two * max(dot(nb, na) * hb - ha, 0.0);
    let wb = two * max(dot(na, nb) * ha - hb, 0.0);
    // The sector's stop arms, a bit each.
    var stops = 0u;
    var d = M_FAR;
    for (var k = 0u; k < n; k++) {
        let range = mw(m_arm(jr, k) + 1u);
        if ((range & 0xffffu) == (range >> 16u)) {
            continue;
        }
        if ((mw(m_arm(jr, k)) & M_ARM_STOP) != 0u) {
            let u = m_arm_dir(jr, k);
            stops |= select(0u, 1u << min(k, 31u), m_wrap(atan2(u.y, u.x) - ta) < behind + ahead);
        } else {
            let side = select(select(0.0, -1.0, k == b), 1.0, k == a) * two;
            d = min(d, m_stroke(block, jr, k, q, side, select(wb, wa, k == a)));
        }
    }
    if (stops == 0u) {
        return keep;
    }
    // The mitre: each facing edge (widened) goes straight on past the node until it meets the other's, behind both
    // arms' node lines; one route arm ends flat at the node.
    if (a != b) {
        let edges = max(dot(v, na) - ha - wa, dot(v, nb) - hb - wb);
        d = min(d, max(max(edges, length(v) - MITRE_LIMIT * max(ha, hb)), max(dot(v, ua), dot(v, ub))));
    }
    if (d * mpu <= ROUTE_EPS_M) {
        return keep;
    }
    return m_junction_frame(block, jr, q, stops, M_NONE, mpu);
}
