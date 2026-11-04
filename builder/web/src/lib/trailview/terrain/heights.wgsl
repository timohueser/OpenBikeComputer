// Heights of the one road model (prototype): `faces::heights::height_at` of the crate, line for line, in
// f32. Per point, each line near it gives its own surface, its distance past its core and a presence;
// the paved field is a normalized priority painter of these surfaces gated by their agreement with the
// owner's; slopes are fill and cut cones from each line's core outline, rounded at their foot, released
// at the reach, and the ground is a smooth min(cut, max(T, fill)). model.wgsl comes first.

const H_SLOTS: u32 = 8u;
const H_BLEND_STEEP: f32 = 1.5;
const H_HEALTH_MARGIN: f32 = 0.25;
const H_HEALTH_WIDTH: f32 = 1.0;
const H_FADE_SHARE: f32 = 0.3;
const H_FADE_GAP: f32 = 1.0;
const H_TOE: f32 = 0.6;
const H_FEATHER_MIN: f32 = 1.0;
const H_ROUND: f32 = 0.5;
const H_BIG: f32 = 1e9;

// The kind of rule that gave a height, as heights::Kind.
const H_TERRAIN: f32 = 0.0;
const H_SLOPE: f32 = 1.0;
const H_PAVED: f32 = 2.0;

fn h_smooth(x: f32) -> f32 {
    let c = clamp(x, 0.0, 1.0);
    return c * c * (3.0 - 2.0 * c);
}

fn h_phi(x: f32) -> f32 {
    return 1.0 - h_smooth(x);
}

fn h_smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.0) {
        return min(a, b);
    }
    let h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

fn h_smax(a: f32, b: f32, k: f32) -> f32 {
    return -h_smin(-a, -b, k);
}

fn h_unit(a: vec2f) -> vec2f {
    let n = length(a);
    return select(vec2f(1.0, 0.0), a / n, n > 0.0);
}

struct HLine {
    first: u32,
    count: u32,
    cut: f32,
    fill: f32,
    reach: f32,
    rank: u32,
    side: f32,
    ret: f32,
}

fn h_line(block: u32, l: u32) -> HLine {
    let at = block + mw(block + 6u) + M_LINE * l;
    return HLine(mw(at + 2u), mw(at + 3u), mf(at + 4u), mf(at + 5u), mf(at + 6u), mw(at + 7u), mf(at + 8u) + mf(at + 9u), mf(at + 10u));
}

struct HSample {
    p: vec2f,
    h: f32,
    z: f32,
    bank: f32,
    grade: f32,
}

fn h_sample(block: u32, ln: HLine, i: u32, mpu: f32) -> HSample {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return HSample(vec2f(mi(at), mi(at + 1u)), mf(at + 2u) * mpu, mf(at + 5u), mf(at + 6u), mf(at + 7u));
}

fn h_pos(block: u32, ln: HLine, i: u32) -> vec2f {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return vec2f(mi(at), mi(at + 1u));
}

// The mitre direction at sample i: the bisector of its two chords.
fn h_mitre(block: u32, ln: HLine, i: u32) -> vec2f {
    var d = vec2f(0.0);
    let p = h_pos(block, ln, i);
    if (i > 0u) {
        d += h_unit(p - h_pos(block, ln, i - 1u));
    }
    if (i + 1u < ln.count) {
        d += h_unit(h_pos(block, ln, i + 1u) - p);
    }
    return h_unit(d);
}

// Where the mitre lines of samples i and i+1 cross: the signed offset along i's left normal.
fn h_crossing(block: u32, ln: HLine, i: u32) -> f32 {
    let a = h_pos(block, ln, i);
    let b = h_pos(block, ln, i + 1u);
    let ta = h_mitre(block, ln, i);
    let tb = h_mitre(block, ln, i + 1u);
    let ma = vec2f(ta.y, -ta.x);
    let mb = vec2f(tb.y, -tb.x);
    let den = m_cross(ma, mb);
    if (abs(den) < 1e-12) {
        return H_BIG;
    }
    return m_cross(b - a, mb) / den;
}

fn h_rho(block: u32, ln: HLine, i: u32, side: f32) -> f32 {
    var r = H_BIG;
    if (i > 0u) {
        let c = h_crossing(block, ln, i - 1u) * side;
        if (c > 0.0) {
            r = min(r, c);
        }
    }
    if (i + 1u < ln.count) {
        let c = h_crossing(block, ln, i) * side;
        if (c > 0.0) {
            r = min(r, c);
        }
    }
    return r;
}

fn h_hermite(z0: f32, z1: f32, m0: f32, m1: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * z0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * z1 + (t3 - t2) * m1;
}

struct HFrame {
    ok: bool,
    z: f32,
    delta: f32,
    d: f32,
    nu: f32,
}

// Segment i of a line at q: in its mitre quad, or past the line's first or last sample.
fn h_segment(block: u32, ln: HLine, i: u32, q: vec2f, mpu: f32) -> HFrame {
    var f = HFrame(false, 0.0, 0.0, 0.0, 0.0);
    let sa = h_sample(block, ln, i, mpu);
    let sb = h_sample(block, ln, i + 1u, mpu);
    let ta = h_mitre(block, ln, i);
    let tb = h_mitre(block, ln, i + 1u);
    let da = dot(q - sa.p, ta);
    let db = dot(sb.p - q, tb);
    let first = i == 0u;
    let last = i + 2u == ln.count;
    let ab = sb.p - sa.p;
    let len = length(ab);
    if (len <= 0.0) {
        return f;
    }
    let u = ab / len;
    let lateral = -m_cross(u, q - sa.p) * mpu;
    let len_m = len * mpu;
    var t = 0.0;
    if (da >= 0.0 && db >= 0.0) {
        t = select(0.5, da / (da + db), da + db > 0.0);
        f.z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
        let g = clamp(dot(q - sa.p, u) / len, 0.0, 1.0);
        let seg = length(q - (sa.p + g * ab)) * mpu;
        f.delta = select(-seg, seg, lateral >= 0.0);
        let h = sa.h + t * (sb.h - sa.h);
        f.d = max(seg - h - ln.side, 0.0);
        let side = select(-1.0, 1.0, lateral >= 0.0);
        let r = ((1.0 - t) * h_rho(block, ln, i, side) + t * h_rho(block, ln, i + 1u, side)) * mpu;
        f.nu = h_smooth((r - seg - H_HEALTH_MARGIN) / H_HEALTH_WIDTH);
    } else if (first && da < 0.0 && db >= 0.0) {
        let past = -dot(q - sa.p, u) * mpu;
        t = 0.0;
        f.z = sa.z - sa.grade * past;
        f.delta = lateral;
        f.d = max(sqrt(lateral * lateral + past * past) - sa.h - ln.side, 0.0);
        f.nu = 1.0;
    } else if (last && db < 0.0 && da >= 0.0) {
        let past = dot(q - sb.p, u) * mpu;
        t = 1.0;
        f.z = sb.z + sb.grade * past;
        f.delta = lateral;
        f.d = max(sqrt(lateral * lateral + past * past) - sb.h - ln.side, 0.0);
        f.nu = 1.0;
    } else {
        return f;
    }
    f.z += (sa.bank + t * (sb.bank - sa.bank)) * f.delta;
    f.ok = true;
    return f;
}

struct HRec {
    line: u32,
    rank: u32,
    z: f32,
    d: f32,
    nu: f32,
    out: f32,
    feather: f32,
    reach: f32,
    ret: f32,
    cut: f32,
    fill: f32,
}

struct HPoint {
    recs: array<HRec, H_SLOTS>,
    n: u32,
    // rel[i * H_SLOTS + j]: how much a fan relates records i and j at the point.
    rel: array<f32, 64>,
}

fn h_present(block: u32, q: vec2f, mpu: f32) -> HPoint {
    var hp: HPoint;
    hp.n = 0u;
    for (var k = 0u; k < 64u; k++) {
        hp.rel[k] = 0.0;
    }
    let bin = vec2i(floor(q / M_BIN));
    if (any(bin < vec2i(0)) || any(bin >= vec2i(M_BINS))) {
        return hp;
    }
    let head = block + mw(block + 9u) + M_HEADER * u32(bin.y * M_BINS + bin.x);
    if (mw(head) == 0u) {
        return hp;
    }
    var at = block + mw(head);
    let counts = mw(head + 1u);
    let hcounts = mw(head + 2u);
    for (var c = 0u; c < (counts >> 16u); c++) {
        at += 2u + 2u * mw(at + 1u);
    }
    at += counts & 0xffffu;
    let nh = hcounts >> 16u;
    let nd = hcounts & 0xffffu;
    let ds = at + nh;
    var k = 0u;
    loop {
        if (k >= nh) {
            break;
        }
        let l = mw(at + k) & 0xffffu;
        let ln = h_line(block, l);
        var best = HFrame(false, 0.0, 0.0, 0.0, 0.0);
        loop {
            if (k >= nh || (mw(at + k) & 0xffffu) != l) {
                break;
            }
            let i = mw(at + k) >> 16u;
            k++;
            let f = h_segment(block, ln, i, q, mpu);
            if (f.ok && f.nu > 0.0 && (!best.ok || abs(f.delta) < abs(best.delta))) {
                best = f;
            }
        }
        if (!best.ok) {
            continue;
        }
        let feather = max(ln.reach, H_FEATHER_MIN);
        if (best.d >= max(feather, ln.reach) || hp.n == H_SLOTS) {
            continue;
        }
        hp.recs[hp.n] = HRec(l, ln.rank, best.z, best.d, best.nu, best.d, feather, ln.reach, ln.ret, ln.cut, ln.fill);
        hp.n++;
    }
    // The painter's order: class rank, then the line (insertion sort).
    for (var i = 1u; i < hp.n; i++) {
        var j = i;
        loop {
            if (j == 0u) {
                break;
            }
            let a = hp.recs[j - 1u];
            let b = hp.recs[j];
            if (a.rank < b.rank || (a.rank == b.rank && a.line <= b.line)) {
                break;
            }
            hp.recs[j - 1u] = b;
            hp.recs[j] = a;
            j--;
        }
    }
    // The fans: the core arc of a corner, and the relation of its two arms.
    let extras = m_extras(block);
    for (var e = 0u; e < nd; e++) {
        let item = mw(ds + e);
        let cand = m_cand(block, item & 0xffffu);
        let dk = extras + mw(cand + 5u) + M_DISC * (item >> 16u);
        let c = vec2f(mi(dk), mi(dk + 1u));
        let radius = mf(dk + 2u) * mpu;
        let u0 = vec2f(mi(dk + 3u), mi(dk + 4u)) - c;
        let u1 = vec2f(mi(dk + 5u), mi(dk + 6u)) - c;
        let v = q - c;
        let l0 = mw(dk + 9u);
        let l1 = mw(dk + 10u);
        let dist = length(v) * mpu;
        if (l0 != M_NONE && l1 != M_NONE && l0 != l1) {
            var i0 = H_SLOTS;
            var i1 = H_SLOTS;
            for (var r = 0u; r < hp.n; r++) {
                if (hp.recs[r].line == l0) {
                    i0 = r;
                }
                if (hp.recs[r].line == l1) {
                    i1 = r;
                }
            }
            if (i0 < H_SLOTS && i1 < H_SLOTS) {
                let reach = max(hp.recs[i0].feather, hp.recs[i1].feather);
                let w = h_phi((dist - radius - (reach - 1.0)) / 1.5);
                hp.rel[i0 * H_SLOTS + i1] = max(hp.rel[i0 * H_SLOTS + i1], w);
                hp.rel[i1 * H_SLOTS + i0] = max(hp.rel[i1 * H_SLOTS + i0], w);
            }
        }
        let turn = m_cross(u0, u1);
        if (m_cross(u0, v) * turn < 0.0 || m_cross(v, u1) * turn < 0.0 || dot(v, u0 + u1) < 0.0 || dist >= radius) {
            continue;
        }
        let whole = atan2(abs(turn), dot(u0, u1));
        let part = atan2(abs(m_cross(u0, v)), dot(u0, v));
        let f = select(0.0, clamp(part / whole, 0.0, 1.0), whole > 0.0);
        let ring = mf(dk + 7u) + f * (mf(dk + 8u) - mf(dk + 7u));
        let d = max(radius - ring - dist, 0.0);
        for (var r = 0u; r < hp.n; r++) {
            let mine = hp.recs[r].line == l0 || hp.recs[r].line == l1 || (l0 == M_NONE && l1 == M_NONE);
            if (mine && d < hp.recs[r].out) {
                hp.recs[r].out = d;
            }
        }
    }
    return hp;
}

// The paved field for owner o: the normalized priority painter of the records that agree with o.
fn h_paved(hp: ptr<function, HPoint>, o: u32) -> f32 {
    let ro = (*hp).recs[o];
    var sw = 0.0;
    var swz = 0.0;
    var keep = 1.0;
    for (var k = 0u; k < (*hp).n; k++) {
        let r = (*hp).recs[k];
        let beta = h_phi(r.d / r.feather) * r.nu;
        var c = 1.0;
        if (k != o) {
            var upper_feather = ro.feather;
            var lower_ret = r.ret;
            if (k < o) {
                upper_feather = r.feather;
                lower_ret = ro.ret;
            }
            let wall = lower_ret * upper_feather / H_BLEND_STEEP;
            let ok = wall * 0.5;
            c = max(h_phi((abs(r.z - ro.z) - ok) / (wall - ok)), (*hp).rel[o * H_SLOTS + k]);
        }
        let w = beta * c * keep;
        sw += w;
        swz += w * r.z;
        keep *= 1.0 - beta * c;
    }
    return select(ro.z, swz / sw, sw > 0.0);
}

fn h_cone(s: f32, d: f32) -> f32 {
    if (d < H_ROUND) {
        return s * d * d / (2.0 * H_ROUND);
    }
    return s * (d - H_ROUND * 0.5);
}

fn h_slope(hp: ptr<function, HPoint>, t: f32) -> f32 {
    var lo = -H_BIG;
    var hi = H_BIG;
    var near = H_BIG;
    var seen = false;
    for (var k = 0u; k < (*hp).n; k++) {
        let r = (*hp).recs[k];
        if (r.reach <= 0.0) {
            continue;
        }
        seen = true;
        let d = r.out;
        near = min(near, d);
        let base = h_paved(hp, k);
        let w = r.reach * H_FADE_SHARE;
        let fade = max(h_smooth((d - (r.reach - w)) / w), 1.0 - r.nu);
        let relax = H_FADE_GAP * fade / max(1.0 - fade, 1e-6);
        lo = max(lo, base - h_cone(r.fill, d) - relax);
        hi = min(hi, base + h_cone(r.cut, d) + relax);
    }
    if (!seen) {
        return t;
    }
    let k = H_TOE * clamp(near / (2.0 * H_TOE), 0.0, 1.0);
    return h_smin(hi, h_smax(t, lo, k), k);
}

// The height at q (frame units from the cell's origin) of the block, with the label there (M_NONE for
// terrain) and the plain terrain t: (height, kind).
fn height_at(block: u32, q: vec2f, label: u32, t: f32) -> vec2f {
    if (block == 0u || label == M_NONE) {
        return vec2f(t, H_TERRAIN);
    }
    let lw = m_label(block, label);
    let rule = mw(lw) >> 24u;
    if (rule == 0u) {
        return vec2f(t, H_TERRAIN);
    }
    let mpu = mf(block + 10u);
    var hp = h_present(block, q, mpu);
    if (rule == 1u) {
        return vec2f(h_slope(&hp, t), H_SLOPE);
    }
    let material = mw(lw + 2u);
    var o = H_SLOTS;
    for (var k = 0u; k < hp.n; k++) {
        if (hp.recs[k].line == material) {
            o = k;
            break;
        }
    }
    if (o == H_SLOTS) {
        for (var k = 0u; k < hp.n; k++) {
            if (hp.recs[k].d <= 0.0) {
                o = k;
                break;
            }
        }
    }
    if (o == H_SLOTS) {
        var best = H_BIG;
        for (var k = 0u; k < hp.n; k++) {
            if (hp.recs[k].d < best) {
                best = hp.recs[k].d;
                o = k;
            }
        }
    }
    if (o == H_SLOTS) {
        return vec2f(t, H_PAVED);
    }
    return vec2f(h_paved(&hp, o), H_PAVED);
}
