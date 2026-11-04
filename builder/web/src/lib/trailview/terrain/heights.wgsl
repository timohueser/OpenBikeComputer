// Heights of the one road model (prototype): `faces::heights::height_at` of the crate, line for line, in
// f32. Per point, each line near it gives its own surface, its distance past its core and a presence;
// the paved field is a normalized painter of these surfaces in which each pair of records blends as far
// as they agree and the greater one rules inside its core; slopes are fill and cut cones from each line's core outline, rounded at their foot, released
// at the reach, and the ground is a smooth min(cut, max(T, fill)). model.wgsl comes first.
// #include model.wgsl

const H_SLOTS: u32 = 8u;
const H_QUADS: u32 = 4u;
const H_LEG_BLEND: f32 = 0.25;
const H_BLEND_STEEP: f32 = 1.5;
const H_HEALTH_MARGIN: f32 = 0.25;
const H_HEALTH_WIDTH: f32 = 1.0;
const H_FADE_SHARE: f32 = 0.3;
const H_FADE_GAP: f32 = 1.0;
const H_TOE_PER_M: f32 = 1.0;
const H_TOE_MAX: f32 = 5.0;
const H_FEATHER_MIN: f32 = 1.0;
const H_ROUND: f32 = 0.5;
const H_BAND_FADE: f32 = 1.0;
const H_BIG: f32 = 1e9;
// SCRATCH, not for merge: the two options of the owner's question 4 (two legs of one way at different
// heights), switched by the probe bits. Both give a line one cone pair from its own surface; R blends the
// legs in the 0.25 m leg ramp, W takes the nearer leg alone and marks the step between them as a wall.
const H_Q4_RAMP: u32 = 65536u;
const H_Q4_WALL: u32 = 131072u;
// W: a wall within this many metres of the middle between two legs that differ by more than H_WALL_DZ.
const H_WALL_NEAR: f32 = 0.35;
const H_WALL_DZ: f32 = 0.5;

// The kind of rule that gave a height, as heights::Kind.
const H_TERRAIN: f32 = 0.0;
const H_SLOPE: f32 = 1.0;
const H_PAVED: f32 = 2.0;
const H_WALL: f32 = 3.0;

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
        // Interpolated as a curvature, so a straight neighbour (no crossing) does not swamp it.
        let r = mpu / ((1.0 - t) / h_rho(block, ln, i, side) + t / h_rho(block, ln, i + 1u, side));
        f.nu = h_smooth((r - seg - H_HEALTH_MARGIN) / H_HEALTH_WIDTH);
    } else if (first && da < 0.0 && db >= 0.0) {
        let past = -dot(q - sa.p, u) * mpu;
        t = 0.0;
        f.z = sa.z - sa.grade * past;
        f.delta = lateral;
        f.d = max(sqrt(lateral * lateral + past * past) - sa.h - ln.side, 0.0);
        // The end vertex's health, as the first quad has it on the end's mitre line.
        let side = select(-1.0, 1.0, lateral >= 0.0);
        f.nu = h_smooth((h_rho(block, ln, i, side) * mpu - abs(lateral) - H_HEALTH_MARGIN) / H_HEALTH_WIDTH);
    } else if (last && db < 0.0 && da >= 0.0) {
        let past = dot(q - sb.p, u) * mpu;
        t = 1.0;
        f.z = sb.z + sb.grade * past;
        f.delta = lateral;
        f.d = max(sqrt(lateral * lateral + past * past) - sb.h - ln.side, 0.0);
        let side = select(-1.0, 1.0, lateral >= 0.0);
        f.nu = h_smooth((h_rho(block, ln, i + 1u, side) * mpu - abs(lateral) - H_HEALTH_MARGIN) / H_HEALTH_WIDTH);
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
    // The quads of the line that hold the point: (own height, metres past the core, health).
    quads: array<vec3f, H_QUADS>,
    nq: u32,
    // W: 1 where the point lies at the step between two legs of the line.
    wall: f32,
}

struct HPoint {
    recs: array<HRec, H_SLOTS>,
    n: u32,
    // rel[i * H_SLOTS + j]: how much a fan relates records i and j at the point.
    rel: array<f32, 64>,
    // The bin's height segment items: first word and count.
    hs: u32,
    nh: u32,
}

// Segment i of a line at its point nearest q: (metres to it, metres past the core there, the
// segment's surface there, its band's support: half-width, side and reach).
fn h_nearest(block: u32, ln: HLine, i: u32, q: vec2f, mpu: f32) -> vec4f {
    let sa = h_sample(block, ln, i, mpu);
    let sb = h_sample(block, ln, i + 1u, mpu);
    let ab = sb.p - sa.p;
    let l2 = dot(ab, ab);
    let f = select(0.0, clamp(dot(q - sa.p, ab) / l2, 0.0, 1.0), l2 > 0.0);
    let dist = length(q - (sa.p + f * ab)) * mpu;
    let h = sa.h + f * (sb.h - sa.h);
    let len_m = sqrt(l2) * mpu;
    let side = select(-1.0, 1.0, m_cross(ab, q - sa.p) <= 0.0);
    let z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, f) + (sa.bank + f * (sb.bank - sa.bank)) * side * dist;
    return vec4f(dist, dist - h - ln.side, z, h + ln.side + ln.reach);
}

// The distance from v (relative to an arc's centre) to the arc of radius r from direction a to
// direction c (the shorter way, under a half turn): radial where v points between them, else to the
// nearer end.
fn h_arc_distance(v: vec2f, a: vec2f, c: vec2f, r: f32) -> f32 {
    let turn = m_cross(a, c);
    if (m_cross(a, v) * turn >= 0.0 && m_cross(v, c) * turn >= 0.0 && dot(v, a + c) >= 0.0) {
        return abs(length(v) - r);
    }
    return min(length(v - h_unit(a) * r), length(v - h_unit(c) * r));
}

// Metres from q to the outer edge of the slope band, from the edges of q's bin (clipped to the bin
// grown by 3 m): exact within 3 m, else 3 m or more.
fn h_slope_edge(block: u32, q: vec2f, mpu: f32) -> f32 {
    let bin = vec2i(floor(q / M_BIN));
    if (any(bin < vec2i(0)) || any(bin >= vec2i(M_BINS))) {
        return H_BIG;
    }
    let head = block + mw(block + 9u) + M_HEADER * u32(bin.y * M_BINS + bin.x);
    if (mw(head) == 0u) {
        return H_BIG;
    }
    var at = block + mw(head);
    let rq = q - (vec2f(bin) * M_BIN + 0.5 * M_BIN);
    var best = H_BIG;
    for (var c = 0u; c < (mw(head + 1u) >> 16u); c++) {
        let ci = mw(at) & 0xffffu;
        let n = mw(at + 1u);
        let k = m_cand(block, ci);
        let label = mw(k + 3u);
        if (mw(k) == M_BAND && label != M_NONE && (mw(m_label(block, label)) & 0xffu) == 1u) {
            for (var e = 0u; e < n; e++) {
                best = min(best, m_segment(m_pair(mw(at + 2u + 2u * e)), m_pair(mw(at + 3u + 2u * e)), rq));
            }
        }
        at += 2u + 2u * n;
    }
    return best * mpu;
}

fn h_present(block: u32, q: vec2f, mpu: f32) -> HPoint {
    var hp: HPoint;
    hp.n = 0u;
    hp.nh = 0u;
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
    hp.hs = at;
    hp.nh = nh;
    var k = 0u;
    loop {
        if (k >= nh) {
            break;
        }
        let l = mw(at + k) & 0xffffu;
        let ln = h_line(block, l);
        var quads: array<vec3f, H_QUADS>;
        var deltas: array<f32, H_QUADS>;
        var nq = 0u;
        // Every listed segment by its nearest point: the least distance, and the fallback surface (a
        // soft nearest of the segments' surfaces at their nearest points, kept as sums relative to the
        // least so far) where no healthy quad is near.
        var least = H_BIG;
        var fb_w = 0.0;
        var fb_z = 0.0;
        var core_d = H_BIG;
        loop {
            if (k >= nh || (mw(at + k) & 0xffffu) != l) {
                break;
            }
            let i = mw(at + k) >> 16u;
            k++;
            let f = h_segment(block, ln, i, q, mpu);
            if (f.ok && f.nu > 0.0 && nq < H_QUADS) {
                quads[nq] = vec3f(f.z, f.d, f.nu);
                deltas[nq] = abs(f.delta);
                nq++;
            }
            let n = h_nearest(block, ln, i, q, mpu);
            core_d = min(core_d, n.y);
            if (n.x < least) {
                let rescale = exp(-(least - n.x) / H_LEG_BLEND);
                fb_w *= rescale;
                fb_z *= rescale;
                least = n.x;
            }
            let psi = exp(-(n.x - least) / H_LEG_BLEND) * h_phi(n.x / n.w);
            fb_w += psi;
            fb_z += psi * n.z;
        }
        core_d = max(core_d, 0.0);
        let feather = max(ln.reach, H_FEATHER_MIN);
        if (core_d >= max(feather, ln.reach) || hp.n == H_SLOTS || (nq == 0u && fb_w <= 0.0)) {
            continue;
        }
        // The line's surface: each healthy quad weighs its health times exp(−(its distance − the
        // least) / H_LEG_BLEND), so a far leg weighs nothing; the fallback takes the rest of 1.
        var sw = 0.0;
        var z = 0.0;
        var jn = 0u;
        for (var j = 1u; j < nq; j++) {
            if (deltas[j] < deltas[jn]) {
                jn = j;
            }
        }
        var wall = 0.0;
        let hard = (MODEL_PROBE & H_Q4_WALL) != 0u;
        for (var j = 0u; j < nq; j++) {
            var a = quads[j].z * exp(-max(deltas[j] - least, 0.0) / H_LEG_BLEND);
            if (hard) {
                a = select(0.0, quads[j].z, j == jn);
                if (j != jn && deltas[j] - deltas[jn] < 2.0 * H_WALL_NEAR && abs(quads[j].x - quads[jn].x) > H_WALL_DZ) {
                    wall = 1.0;
                }
            }
            sw += a;
            z += a * quads[j].x;
        }
        let fb = max(1.0 - sw, 0.0);
        let zf = select(0.0, fb_z / fb_w, fb_w > 0.0);
        // The distance past the core is the plain one to the nearest segment: 0 exactly inside.
        hp.recs[hp.n] = HRec(l, ln.rank, (z + fb * zf) / (sw + fb), core_d, min(sw, 1.0), H_BIG, feather, ln.reach, ln.ret, ln.cut, ln.fill, quads, nq, wall);
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
        // The ring's outer arc lies at the wider side's ring; at a narrower side it tapers back to
        // that arm's own band along a circle of the wider ring about the foot (GP3's band rings).
        let ring0 = mf(dk + 7u);
        let ring1 = mf(dk + 8u);
        let wide = max(ring0, ring1);
        // The corner's outline: the arc of radius r − ring between the feet, and at a narrower foot
        // GP3's taper, an arc of the wider ring about the foot from the ring's end to that arm's own
        // band. The distance is the unsigned distance to these arcs: continuous everywhere.
        var d = h_arc_distance(v, u0, u1, max(radius - wide, 0.0) / mpu) * mpu;
        for (var side = 0u; side < 2u; side++) {
            let foot = select(u1, u0, side == 0u);
            let other = select(u0, u1, side == 0u);
            let own = select(ring1, ring0, side == 0u);
            if (own >= wide) {
                continue;
            }
            let n = h_unit(-foot);
            var u = vec2f(n.y, -n.x);
            if (dot(u, other - foot) > 0.0) {
                u = -u;
            }
            let end_angle = acos(own / wide);
            let end = cos(end_angle) * n + sin(end_angle) * u;
            d = min(d, h_arc_distance(v - foot, n, end, wide / mpu) * mpu);
        }
        for (var r = 0u; r < hp.n; r++) {
            let mine = hp.recs[r].line == l0 || hp.recs[r].line == l1;
            if (mine && d < hp.recs[r].out) {
                hp.recs[r].out = d;
            }
        }
    }
    return hp;
}

// The paved field for owner o: the normalized priority painter of the records that agree with o.
fn h_paved(hp: ptr<function, HPoint>, o: u32) -> f32 {
    return h_paved_as(hp, o, (*hp).recs[o].z, (*hp).recs[o].d);
}

// The paved field for owner o with o's own height and distance taken from one of its quads.
fn h_paved_as(hp: ptr<function, HPoint>, o: u32, zo: f32, dov: f32) -> f32 {
    let n = (*hp).n;
    var z: array<f32, H_SLOTS>;
    var d: array<f32, H_SLOTS>;
    var beta: array<f32, H_SLOTS>;
    for (var k = 0u; k < n; k++) {
        z[k] = select((*hp).recs[k].z, zo, k == o);
        d[k] = select((*hp).recs[k].d, dov, k == o);
        beta[k] = h_phi(d[k] / (*hp).recs[k].feather);
    }
    // A greater record i rules a lesser j as far as they agree; inside i's core it rules it fully.
    // A lesser record j keeps a greater i off its ground as far as they disagree, except inside i's
    // core. So agreeing records blend, and disagreeing ones meet in a step on the greater one's core
    // edge.
    var sw = 0.0;
    var swz = 0.0;
    for (var j = 0u; j < n; j++) {
        var w = beta[j];
        for (var i = 0u; i < n; i++) {
            if (i == j) {
                continue;
            }
            // The pair's agreement: a fan relates them, or the rise between them is one that the
            // lesser one's return grade climbs over the greater one's feather.
            let upper = min(i, j);
            let lower = max(i, j);
            let wall = (*hp).recs[lower].ret * (*hp).recs[upper].feather / H_BLEND_STEEP;
            let ok = wall * 0.5;
            let c = max(h_phi((abs(z[i] - z[j]) - ok) / (wall - ok)), (*hp).rel[i * H_SLOTS + j]);
            if (i < j) {
                w *= select(1.0 - beta[i] * c, 0.0, d[i] <= 0.0);
            } else if (d[j] > 0.0) {
                w *= 1.0 - beta[i] * (1.0 - c);
            }
        }
        sw += w;
        swz += w * z[j];
    }
    return select(z[o], swz / sw, sw > 0.0);
}

fn h_cone(s: f32, d: f32) -> f32 {
    if (d < H_ROUND) {
        return s * d * d / (2.0 * H_ROUND);
    }
    return s * (d - H_ROUND * 0.5);
}

fn h_slope(block: u32, q: vec2f, mpu: f32, hp: ptr<function, HPoint>, t: f32, edge: f32) -> f32 {
    var lo = -H_BIG;
    var hi = H_BIG;
    var toe = H_TOE_MAX;
    var seen = false;
    // SCRATCH (question 4): a line gives one cone pair, from its own surface and its plain core distance.
    if ((MODEL_PROBE & (H_Q4_RAMP | H_Q4_WALL)) != 0u) {
        for (var k = 0u; k < (*hp).n; k++) {
            let r = (*hp).recs[k];
            if (r.reach <= 0.0) {
                continue;
            }
            seen = true;
            let d = min(r.d, r.out);
            let base = h_paved(hp, k);
            let w = r.reach * H_FADE_SHARE;
            let fade = h_smooth((r.d - (r.reach - w)) / w);
            let relax = H_FADE_GAP * fade / max(1.0 - fade, 1e-6);
            let kq = min(H_TOE_PER_M * d, H_TOE_MAX);
            toe = min(toe, kq + fade * (H_TOE_MAX - kq));
            lo = max(lo, base - h_cone(r.fill, d) - relax);
            hi = min(hi, base + h_cone(r.cut, d) + relax);
        }
        if (!seen) {
            return t;
        }
        let g = h_smin(hi, h_smax(t, lo, toe), toe);
        return t + h_smooth(edge / H_BAND_FADE) * (g - t);
    }
    for (var k = 0u; k < (*hp).n; k++) {
        let r = (*hp).recs[k];
        if (r.reach <= 0.0) {
            continue;
        }
        seen = seen || r.nq > 0u;
        for (var j = 0u; j < r.nq; j++) {
            let qd = r.quads[j];
            let d = min(qd.y, r.out);
            let base = h_paved_as(hp, k, qd.x, qd.y);
            let w = r.reach * H_FADE_SHARE;
            let fade = max(h_smooth((qd.y - (r.reach - w)) / w), 1.0 - qd.z);
            let relax = H_FADE_GAP * fade / max(1.0 - fade, 1e-6);
            let kq = min(H_TOE_PER_M * d, H_TOE_MAX);
            toe = min(toe, kq + fade * (H_TOE_MAX - kq));
            lo = max(lo, base - h_cone(r.fill, d) - relax);
            hi = min(hi, base + h_cone(r.cut, d) + relax);
        }
    }
    // Where a line's frame is not healthy (past the crossing of its mitre lines on a bend's inner
    // side, where no quad holds q), each of its segments gives a cone from its nearest point instead:
    // continuous everywhere, and let go where the frame is healthy.
    for (var k = 0u; k < (*hp).nh; k++) {
        let item = mw((*hp).hs + k);
        let l = item & 0xffffu;
        var ri = H_SLOTS;
        for (var j = 0u; j < (*hp).n; j++) {
            if ((*hp).recs[j].line == l) {
                ri = j;
                break;
            }
        }
        if (ri == H_SLOTS) {
            continue;
        }
        let r = (*hp).recs[ri];
        if (r.nu >= 1.0 || r.reach <= 0.0) {
            continue;
        }
        let n = h_nearest(block, h_line(block, l), item >> 16u, q, mpu);
        let dn = max(n.y, 0.0);
        if (dn >= r.reach) {
            continue;
        }
        let w = r.reach * H_FADE_SHARE;
        let fade = max(h_smooth((dn - (r.reach - w)) / w), r.nu);
        let relax = H_FADE_GAP * fade / max(1.0 - fade, 1e-6);
        let kq = min(H_TOE_PER_M * dn, H_TOE_MAX);
        toe = min(toe, kq + fade * (H_TOE_MAX - kq));
        let d = min(dn, r.out);
        lo = max(lo, n.z - h_cone(r.fill, d) - relax);
        hi = min(hi, n.z + h_cone(r.cut, d) + relax);
        seen = true;
    }
    if (!seen) {
        return t;
    }
    let g = h_smin(hi, h_smax(t, lo, toe), toe);
    return t + h_smooth(edge / H_BAND_FADE) * (g - t);
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
    var walled = false;
    for (var k = 0u; k < hp.n; k++) {
        walled = walled || hp.recs[k].wall > 0.0;
    }
    if (rule == 1u) {
        return vec2f(h_slope(block, q, mpu, &hp, t, h_slope_edge(block, q, mpu)), select(H_SLOPE, H_WALL, walled));
    }
    // The owner: the first record whose core holds q (the shape's order puts carriageways first),
    // else the label's material line, else the nearest. A junction region's points on a guest arm so
    // belong to that arm, not to the region's material line far away.
    let material = mw(lw + 2u);
    var o = H_SLOTS;
    for (var k = 0u; k < hp.n; k++) {
        if (hp.recs[k].d <= 0.0) {
            o = k;
            break;
        }
    }
    if (o == H_SLOTS) {
        for (var k = 0u; k < hp.n; k++) {
            if (hp.recs[k].line == material) {
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
    return vec2f(h_paved(&hp, o), select(H_PAVED, H_WALL, walled));
}
