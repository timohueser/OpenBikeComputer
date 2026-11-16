// The paved outline of a cell's records: the distance from a point to it, as model_point's, and the exact integer
// side tests that the model and the outline share.
// #include records.wgsl

const M_OUTLINE: u32 = 5u;
const M_BIN_UNITS: i32 = 4096;
// A round's chords leave its arc by at most this share of its radius (the overlay's 10° steps, with room).
const M_SAGITTA: f32 = 0.0055;

// The product of two i32 as a signed 64-bit number: (low word, high word).
fn m_mul(a: i32, b: i32) -> vec2u {
    let x = u32(abs(a));
    let y = u32(abs(b));
    let x0 = x & 0xffffu;
    let x1 = x >> 16u;
    let y0 = y & 0xffffu;
    let y1 = y >> 16u;
    let p00 = x0 * y0;
    let p01 = x0 * y1;
    let p10 = x1 * y0;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    var lo = (p00 & 0xffffu) | (mid << 16u);
    var hi = x1 * y1 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    if ((a < 0) != (b < 0)) {
        lo = ~lo + 1u;
        hi = ~hi + select(0u, 1u, lo == 0u);
    }
    return vec2u(lo, hi);
}

// The sign of cross(a, b), exact for components of up to 31 bits.
fn m_orient(a: vec2i, b: vec2i) -> i32 {
    let l = m_mul(a.x, b.y);
    let r = m_mul(a.y, b.x);
    if (l.y != r.y) {
        return select(-1, 1, bitcast<i32>(l.y) > bitcast<i32>(r.y));
    }
    if (l.x != r.x) {
        return select(-1, 1, l.x > r.x);
    }
    return 0;
}

// Whether point p moved by (ε, ε²) lies left of the line through a along e, as records::left.
fn m_left(e: vec2i, a: vec2i, p: vec2i) -> bool {
    let s = m_orient(e, p - a);
    if (s != 0) {
        return s > 0;
    }
    return e.y < 0 || (e.y == 0 && e.x > 0);
}

// The change of the winding number from c to q, both moved by (ε, ε²), across edge ab, as
// records::crossing. In doubled units.
fn m_crossing(a: vec2i, b: vec2i, c: vec2i, q: vec2i) -> i32 {
    // Segments whose boxes lie a unit or more apart do not cross, moved by (ε, ε²) or not.
    if (any(max(a, b) < min(c, q)) || any(min(a, b) > max(c, q))) {
        return 0;
    }
    let e = b - a;
    let sq = m_left(e, a, q);
    if (m_left(e, a, c) == sq) {
        return 0;
    }
    let d = q - c;
    let sa = m_orient(d, a - c);
    let sb = m_orient(d, b - c);
    let tie = d.y > 0 || (d.y == 0 && d.x < 0);
    if (select(sa > 0, tie, sa == 0) == select(sb > 0, tie, sb == 0)) {
        return 0;
    }
    return select(-1, 1, sq);
}

fn m_segment(a: vec2f, b: vec2f, q: vec2f) -> f32 {
    let u = b - a;
    let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
    return length(q - a - t * u);
}

// The outline polygon cuts each round's arc in chords; its discs move the metres `poly` from q to the polygon of the
// outline candidate at word k onto the arcs, by no more than the chords' sagitta.
fn m_on_arcs(block: u32, k: u32, qf: vec2f, poly: f32, mpu: f32) -> f32 {
    var d = poly;
    for (var j = 0u; j < mw(k + 6u); j++) {
        let disc = m_extras(block) + mw(k + 5u) + M_DISC * j;
        let r = mf(disc + 2u);
        let arc = (r - length(qf - vec2f(m_vertex(disc)))) * mpu;
        d = max(d, min(arc, poly + (r * M_SAGITTA + 1.0) * mpu));
    }
    return d;
}

// The metres from q to the paved outline (negative inside), as model_point's before the rail beds: from the bin's
// first item, where it is the outline; M_FAR where the bin lists none.
fn m_outline_metres(block: u32, q: vec2i, mpu: f32) -> f32 {
    let head = m_bin_head(block, vec2f(q));
    if (head == 0u || (mw(head + 1u) >> 16u) == 0u) {
        return M_FAR;
    }
    let at = block + mw(head);
    let k = m_cand(block, mw(at) & 0xffffu);
    if (mw(k) != M_OUTLINE) {
        return M_FAR;
    }
    let bin = min(q / M_BIN_UNITS, vec2i(M_BINS - 1));
    let c = 2 * (bin * M_BIN_UNITS + M_BIN_UNITS / 2) + 1;
    var wind = i32((mw(at) >> 16u) & 0xffu) - 128;
    var d = M_FAR;
    var run = at + 2u;
    for (var r = 0u; r < mw(at + 1u); r++) {
        let n = mw(run);
        for (var e = 0u; e + 1u < n; e++) {
            let a = m_vertex(run + 1u + 2u * e);
            let b = m_vertex(run + 3u + 2u * e);
            wind += m_crossing(2 * a, 2 * b, c, 2 * q);
            d = min(d, m_segment(vec2f(a), vec2f(b), vec2f(q)));
        }
        run += 1u + 2u * n;
    }
    return m_on_arcs(block, k, vec2f(q), select(d, -d, wind != 0) * mpu, mpu);
}

// The metres from q, which lies outside the paved outline, to it, as m_outline_metres for such a point: the least
// distance to the chords of the bin's first item, where it is the outline, moved onto its arcs; no winding. The arcs
// only move it up, so a distance to the chords of `within` or more is returned as it is.
fn m_outline_out(block: u32, q: vec2f, mpu: f32, within: f32) -> f32 {
    let head = m_bin_head(block, q);
    if (head == 0u || (mw(head + 1u) >> 16u) == 0u) {
        return M_FAR;
    }
    let at = block + mw(head);
    let k = m_cand(block, mw(at) & 0xffffu);
    if (mw(k) != M_OUTLINE) {
        return M_FAR;
    }
    var d = M_FAR;
    var run = at + 2u;
    for (var r = 0u; r < mw(at + 1u); r++) {
        let n = mw(run);
        for (var e = 0u; e + 1u < n; e++) {
            d = min(d, m_segment(vec2f(m_vertex(run + 1u + 2u * e)), vec2f(m_vertex(run + 3u + 2u * e)), q));
        }
        run += 1u + 2u * n;
    }
    if (d * mpu >= within) {
        return d * mpu;
    }
    return m_on_arcs(block, k, q, d * mpu, mpu);
}
