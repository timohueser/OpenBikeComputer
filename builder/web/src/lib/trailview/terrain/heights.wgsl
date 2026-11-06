// The heights of the one road model: the crate's `faces::heights::height_at`, line for line, in f32.
// Per point, each line near it gives its own surface, its distance past its core and a presence; the
// paved field is a normalized painter of these surfaces in which each pair of records blends as far as
// they agree and the greater one rules inside its core; slopes are fill and cut cones from each line's
// core outline, released at the reach, and the ground is a smooth min(cut, max(T, fill)).
// #include records.wgsl

const H_SLOTS: u32 = 10u;
const H_PAIRS: u32 = 100u;
const H_QUADS: u32 = 4u;
// The most regions beyond a region's edges that weigh at a point, as heights::REGIONS.
const H_REGIONS: u32 = 2u;
// The heights' rule words follow the block's head (heights::RULES of the crate).
const H_RULES_AT: u32 = 12u;

// The rule values of heights, as heights::HeightRules: read from the block by height_at.
struct HRules {
    feather_min: f32,
    blend_steep: f32,
    leg_reach: f32,
    health_margin: f32,
    health_width: f32,
    frame_tilt: f32,
    round: f32,
    fade_share: f32,
    fade_gap: f32,
    toe_per_m: f32,
    toe_max: f32,
    toe_span: f32,
    toe_gap_min: f32,
    via_fade: f32,
    band_fade: f32,
    mitre_band: f32,
    region_fade: f32,
    legs_wall: bool,
}
var<private> H: HRules;
// The whole metres that height_at takes off every absolute height and adds back at the end: f32 sums of
// heights near the plain terrain keep their sub-millimetre steps on high ground.
var<private> H_BASE: f32;

fn h_rules(block: u32) -> HRules {
    let r = block + H_RULES_AT;
    return HRules(mf(r), mf(r + 1u), mf(r + 2u), mf(r + 3u), mf(r + 4u), mf(r + 5u), mf(r + 6u), mf(r + 7u), mf(r + 8u), mf(r + 9u), mf(r + 10u), mf(r + 11u), mf(r + 12u), mf(r + 13u), mf(r + 14u), mf(r + 15u), mf(r + 17u), mw(r + 18u) == 1u);
}

// An unknown terrain at the reach.
const H_UNKNOWN: f32 = -1e9;
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

// The width of the toe's smooth min/max at d metres past the outline, levelling at `level` or
// H.toe_max, as heights::toe_width.
fn h_toe_width(d: f32, level: f32) -> f32 {
    let most = min(level, H.toe_max);
    let x = min(H.toe_per_m * d / (2.0 * most), 1.0);
    return most * (1.0 - (1.0 - x) * (1.0 - x));
}

// The lesser of two widths with no kink where they cross, as heights::soft_min.
fn h_soft_min(a: f32, b: f32) -> f32 {
    let lo = min(a, b);
    let r = lo / max(a, b);
    return select(lo / sqrt(sqrt(1.0 + r * r * r * r)), lo, lo <= 0.0);
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
    // The class's steepest fill and cut of the no-room rule.
    fill_max: f32,
    cut_max: f32,
}

fn h_line(block: u32, l: u32) -> HLine {
    let at = block + mw(block + 6u) + M_LINE * l;
    return HLine(mw(at + 2u), mw(at + 3u), mf(at + 4u), mf(at + 5u), mf(at + 6u), mw(at + 7u), mf(at + 8u) + mf(at + 9u), mf(at + 10u), mf(at + 14u), mf(at + 15u));
}

struct HSample {
    p: vec2f,
    h: f32,
    z: f32,
    bank: f32,
    grade: f32,
    // The plain terrain at the slope's reach on the left and the right, H_UNKNOWN where unknown.
    reach_t: vec2f,
}

// A height word less H_BASE, H_UNKNOWN where it is unknown.
fn h_known_word(i: u32) -> f32 {
    let w = mw(i);
    return select(bitcast<f32>(w) - H_BASE, H_UNKNOWN, (w & 0x7f800000u) == 0x7f800000u);
}

fn h_sample(block: u32, ln: HLine, i: u32, mpu: f32) -> HSample {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    // The records hold the right first (frame axes, y south).
    return HSample(vec2f(mi(at), mi(at + 1u)), mf(at + 2u) * mpu, mf(at + 5u) - H_BASE, mf(at + 6u), mf(at + 7u), vec2f(h_known_word(at + 9u), h_known_word(at + 8u)));
}

// The terrain at the reach between two samples on one side (0 left, 1 right): unknown if either is.
fn h_reach_t(a: HSample, b: HSample, side: u32, t: f32) -> f32 {
    let x = select(a.reach_t.x, a.reach_t.y, side == 1u);
    let y = select(b.reach_t.x, b.reach_t.y, side == 1u);
    return select(x + t * (y - x), H_UNKNOWN, x <= H_UNKNOWN || y <= H_UNKNOWN);
}

// The plain terrain's grade outward on the cone's side, as heights::terrain_grade.
fn h_terrain_grade(a: HSample, b: HSample, left: f32, t: f32, core: f32, reach: f32) -> f32 {
    let l = h_reach_t(a, b, 0u, t);
    let r = h_reach_t(a, b, 1u, t);
    if (l <= H_UNKNOWN || r <= H_UNKNOWN) {
        return select(0.0, H_UNKNOWN, l <= H_UNKNOWN && r <= H_UNKNOWN);
    }
    return (left * l + (1.0 - left) * r - (left * r + (1.0 - left) * l)) / (2.0 * (core + reach));
}

// The plain terrain where a cone should meet it, with `left` the left side's share, as
// heights::meet_terrain.
fn h_meet_terrain(a: HSample, b: HSample, left: f32, t: f32, core: f32, reach: f32) -> f32 {
    let l0 = h_reach_t(a, b, 0u, t);
    let r0 = h_reach_t(a, b, 1u, t);
    if (l0 <= H_UNKNOWN && r0 <= H_UNKNOWN) {
        return H_UNKNOWN;
    }
    let r = select(r0, l0, r0 <= H_UNKNOWN);
    let l = select(l0, r, l0 <= H_UNKNOWN);
    let side = left * l + (1.0 - left) * r;
    let other = left * r + (1.0 - left) * l;
    return side - (side - other) * H.fade_share * reach / (2.0 * (core + reach));
}

fn h_left_share(lateral: f32, dist: f32) -> f32 {
    return select(0.5, clamp(0.5 + 0.5 * lateral / dist, 0.0, 1.0), dist > 0.0);
}

// The mitre direction at sample i and the health radius on a side (+1 left, −1 right).
fn h_mitre(block: u32, ln: HLine, i: u32) -> vec2f {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return vec2f(mf(at + 10u), mf(at + 11u));
}

fn h_rho(block: u32, ln: HLine, i: u32, side: f32) -> f32 {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return mf(at + select(13u, 12u, side > 0.0));
}

fn h_hermite(z0: f32, z1: f32, m0: f32, m1: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * z0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * z1 + (t3 - t2) * m1;
}

// The frame's health at seg metres from the line toward a health radius r, as heights::health.
fn h_health(r: f32, seg: f32) -> f32 {
    return h_smooth((r - seg - H.health_margin) / H.health_width);
}

// The frame's units across at q in the quad of segment a, a + ab with the mitres ta and tb, at its
// share t (signed, + left), as heights::across.
fn h_across(q: vec2f, a: vec2f, ab: vec2f, ta: vec2f, tb: vec2f, t: f32) -> f32 {
    let m = h_unit(ta + t * (tb - ta));
    let v = q - a;
    let along = dot(ab, m);
    let f = select(0.0, dot(v, m) / along, along != 0.0);
    return -m_cross(m, v - f * ab);
}

struct HFrame {
    ok: bool,
    z: f32,
    delta: f32,
    nu: f32,
    reach_t: f32,
    grade_t: f32,
}

// Segment i of a line at q: in its mitre quad, or past the line's first or last sample, as
// heights::segment_of.
fn h_segment(block: u32, ln: HLine, i: u32, sa: HSample, sb: HSample, q: vec2f, mpu: f32) -> HFrame {
    var f = HFrame(false, 0.0, 0.0, 0.0, H_UNKNOWN, H_UNKNOWN);
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
    var around = 0.0;
    if (da >= 0.0 && db >= 0.0) {
        t = select(0.5, da / (da + db), da + db > 0.0);
        f.z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
        f.delta = h_across(q, sa.p, ab, ta, tb, t) * mpu;
        around = abs(f.delta);
        let rl = mpu / ((1.0 - t) / h_rho(block, ln, i, 1.0) + t / h_rho(block, ln, i + 1u, 1.0));
        let rr = mpu / ((1.0 - t) / h_rho(block, ln, i, -1.0) + t / h_rho(block, ln, i + 1u, -1.0));
        f.nu = h_health(rl, f.delta) * h_health(rr, -f.delta);
    } else if (first && da < 0.0 && db >= 0.0) {
        let past = -dot(q - sa.p, u) * mpu;
        t = 0.0;
        f.z = sa.z - sa.grade * past;
        f.delta = lateral;
        around = sqrt(lateral * lateral + past * past);
        f.nu = h_health(h_rho(block, ln, i, 1.0) * mpu, lateral) * h_health(h_rho(block, ln, i, -1.0) * mpu, -lateral);
    } else if (last && db < 0.0 && da >= 0.0) {
        let past = dot(q - sb.p, u) * mpu;
        t = 1.0;
        f.z = sb.z + sb.grade * past;
        f.delta = lateral;
        around = sqrt(lateral * lateral + past * past);
        f.nu = h_health(h_rho(block, ln, i + 1u, 1.0) * mpu, lateral) * h_health(h_rho(block, ln, i + 1u, -1.0) * mpu, -lateral);
    } else {
        return f;
    }
    let core = sa.h + t * (sb.h - sa.h) + ln.side;
    f.reach_t = h_meet_terrain(sa, sb, h_left_share(f.delta, around), t, core, ln.reach);
    f.grade_t = h_terrain_grade(sa, sb, h_left_share(f.delta, around), t, core, ln.reach);
    // The bank tilts about the frame's metres across, and neighbour quads blend over a band about their
    // mitre line.
    f.z += (sa.bank + t * (sb.bank - sa.bank)) * f.delta;
    if (da >= 0.0 && db >= 0.0) {
        let own = (da + db) * mpu;
        if (i > 0u && da * mpu < min(H.mitre_band, own / 2.0)) {
            f.z = h_mitre_band(f.z, h_surface(block, ln, i - 1u, q, mpu), da * mpu, own);
        }
        if (i + 2u < ln.count && db * mpu < min(H.mitre_band, own / 2.0)) {
            f.z = h_mitre_band(f.z, h_surface(block, ln, i + 1u, q, mpu), db * mpu, own);
        }
    }
    f.ok = true;
    return f;
}

// A quad's surface z at x metres from its mitre line blended with the neighbour's carried on past it
// (n: its surface and its length through q, metres), as heights::mitre_band.
fn h_mitre_band(z: f32, n: vec2f, x: f32, own: f32) -> f32 {
    if (n.y <= 0.0) {
        return z;
    }
    let w = 1.0 / (1.0 / H.mitre_band + 2.0 / own + 2.0 / n.y);
    let s = h_smooth((x / w + 1.0) / 2.0);
    return s * z + (1.0 - s) * n.x;
}

// Segment j's surface at q past its mitre quad and its length through q (0 when not positive), as
// heights::surface.
fn h_surface(block: u32, ln: HLine, j: u32, q: vec2f, mpu: f32) -> vec2f {
    let sa = h_sample(block, ln, j, mpu);
    let sb = h_sample(block, ln, j + 1u, mpu);
    let da = dot(q - sa.p, h_mitre(block, ln, j));
    let db = dot(sb.p - q, h_mitre(block, ln, j + 1u));
    let ab = sb.p - sa.p;
    let len = length(ab);
    if (da + db <= 0.0 || len <= 0.0) {
        return vec2f(0.0, 0.0);
    }
    let t = da / (da + db);
    let len_m = len * mpu;
    let n = h_across(q, sa.p, ab, h_mitre(block, ln, j), h_mitre(block, ln, j + 1u), t) * mpu;
    let z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t) + (sa.bank + t * (sb.bank - sa.bank)) * n;
    return vec2f(z, (da + db) * mpu);
}

struct HRec {
    line: u32,
    rank: u32,
    // The glob order of its first sample: the painter's order among equal classes.
    glob: u32,
    z: f32,
    d: f32,
    out: f32,
    feather: f32,
    reach: f32,
    ret: f32,
    cut: f32,
    fill: f32,
    fill_max: f32,
    cut_max: f32,
    // The plain terrain where the line's cones should meet it, blended as its surface.
    reach_t: f32,
    // The terrain's grade outward, blended as reach_t.
    reach_g: f32,
}

struct HPoint {
    recs: array<HRec, H_SLOTS>,
    n: u32,
    // rel[i * H_SLOTS + j]: how much a fan relates records i and j at the point.
    rel: array<f32, H_PAIRS>,
}

// Whether record a goes before record b in the painter's order: class rank, then the global key.
fn h_before(a: HRec, b: HRec) -> bool {
    return a.rank < b.rank || (a.rank == b.rank && (a.glob < b.glob || (a.glob == b.glob && a.line < b.line)));
}

// Insert a record in the painter's order, as heights::present: past H_SLOTS records the last by it go,
// but never the labels' material lines.
fn h_insert(hp: ptr<function, HPoint>, r: HRec, keep: vec2u) {
    if ((*hp).n == H_SLOTS) {
        var last = H_SLOTS;
        for (var k = 0u; k < H_SLOTS; k++) {
            let l = (*hp).recs[k].line;
            if (l != keep.x && l != keep.y) {
                last = k;
            }
        }
        let kept = r.line == keep.x || r.line == keep.y;
        if (last == H_SLOTS || (!kept && !h_before(r, (*hp).recs[last]))) {
            return;
        }
        for (var k = last; k + 1u < H_SLOTS; k++) {
            (*hp).recs[k] = (*hp).recs[k + 1u];
        }
        (*hp).n--;
    }
    var j = (*hp).n;
    loop {
        if (j == 0u || !h_before(r, (*hp).recs[j - 1u])) {
            break;
        }
        (*hp).recs[j] = (*hp).recs[j - 1u];
        j--;
    }
    (*hp).recs[j] = r;
    (*hp).n++;
}

fn h_glob(block: u32, ln: HLine) -> u32 {
    return mw(block + mw(block + 7u) + M_SAMPLE * ln.first + 4u);
}

// The distance from v (relative to an arc's centre) to the arc of radius r from direction a to
// direction c, as heights::arc_distance.
fn h_arc_distance(v: vec2f, a: vec2f, c: vec2f, r: f32) -> f32 {
    let turn = m_cross(a, c);
    if (m_cross(a, v) * turn >= 0.0 && m_cross(v, c) * turn >= 0.0 && dot(v, a + c) >= 0.0) {
        return abs(length(v) - r);
    }
    return min(length(v - h_unit(a) * r), length(v - h_unit(c) * r));
}

// The distance from q to the segment a, b: q − a first, as heights::to_segment.
fn h_point_segment(a: vec2f, b: vec2f, q: vec2f) -> f32 {
    let u = b - a;
    let l2 = dot(u, u);
    let f = select(0.0, clamp(dot(q - a, u) / l2, 0.0, 1.0), l2 > 0.0);
    return length(q - a - f * u);
}

// Metres from q to the outer edge of the slope band, from the edges of q's bin, as
// heights::slope_edge.
fn h_slope_edge(block: u32, q: vec2f, mpu: f32) -> f32 {
    let head = m_bin_head(block, q);
    if (head == 0u) {
        return H_BIG;
    }
    var at = block + mw(head);
    var best = H_BIG;
    for (var c = 0u; c < (mw(head + 1u) >> 16u); c++) {
        let next = m_skip_item(at);
        let k = m_cand(block, mw(at) & 0xffffu);
        let label = mw(k + 3u);
        if (mw(k) == M_BAND && label != M_NONE && (mw(m_label(block, label)) & 0xffu) == M_SURFACE_SLOPE) {
            var run = at + 2u;
            for (var r = 0u; r < mw(at + 1u); r++) {
                let n = mw(run);
                for (var e = 0u; e + 1u < n; e++) {
                    let a = vec2f(mi(run + 1u + 2u * e), mi(run + 2u + 2u * e));
                    let b = vec2f(mi(run + 3u + 2u * e), mi(run + 4u + 2u * e));
                    best = min(best, h_point_segment(a, b, q));
                }
                run += 1u + 2u * n;
            }
        }
        at = next;
    }
    return best * mpu;
}

// The first word of q's bin's height segment items, as heights::bin_items.
fn h_heights_start(block: u32, q: vec2f) -> u32 {
    let segs = m_bin_segments(block, q);
    return select(segs.x + segs.y, 0u, segs.x == 0u);
}

// The lines present at q, as heights::present. keep: the material lines of the labels asked for,
// never skipped as absent.
fn h_present(block: u32, q: vec2f, mpu: f32, keep: vec2u, start: u32) -> HPoint {
    var hp: HPoint;
    hp.n = 0u;
    for (var k = 0u; k < H_PAIRS; k++) {
        hp.rel[k] = 0.0;
    }
    let head = m_bin_head(block, q);
    if (head == 0u) {
        return hp;
    }
    let hcounts = mw(head + 2u);
    let at = select(h_heights_start(block, q), start, start != 0u);
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
        // A line whose core lies a feather or more from q is not present: skip it before its frames
        // (the labels' material lines stay).
        let feather = max(ln.reach, H.feather_min);
        let kept = l == keep.x || l == keep.y;
        var end = k;
        var near = H_BIG;
        var least = H_BIG;
        loop {
            if (end >= nh || (mw(at + end) & 0xffffu) != l) {
                break;
            }
            end++;
            let wa = block + mw(block + 7u) + M_SAMPLE * (ln.first + (mw(at + end - 1u) >> 16u));
            let pa = vec2f(mi(wa), mi(wa + 1u));
            let ab = vec2f(mi(wa + M_SAMPLE), mi(wa + M_SAMPLE + 1u)) - pa;
            let l2 = dot(ab, ab);
            let f = select(0.0, clamp(dot(q - pa, ab) / l2, 0.0, 1.0), l2 > 0.0);
            let half = (mf(wa + 2u) + f * (mf(wa + M_SAMPLE + 2u) - mf(wa + 2u))) * mpu;
            let dist = length(q - pa - f * ab) * mpu;
            near = min(near, dist - half - ln.side);
            least = min(least, dist);
        }
        if (near >= feather && !kept) {
            k = end;
            continue;
        }
        // The healthy quads: (own height, health, terrain at the reach, its grade outward).
        var quads: array<vec4f, H_QUADS>;
        var deltas: array<f32, H_QUADS>;
        var nq = 0u;
        // The least frame metres across of the line's quads that hold q so far, healthy or not.
        var across = H_BIG;
        // The fallback surface: a soft nearest of the segments' surfaces at their nearest points.
        var fb_w = 0.0;
        var fb_z = 0.0;
        var fb_tw = 0.0;
        var fb_t = 0.0;
        var fb_g = 0.0;
        loop {
            if (k >= end) {
                break;
            }
            let i = mw(at + k) >> 16u;
            k++;
            let sa = h_sample(block, ln, i, mpu);
            let sb = h_sample(block, ln, i + 1u, mpu);
            let f = h_segment(block, ln, i, sa, sb, q, mpu);
            // The quads that can weigh, in the order of the line's samples, as heights::present: when
            // the slots are full, those that a nearer quad put past the leg's reach go.
            if (f.ok) {
                across = min(across, abs(f.delta));
            }
            if (f.ok && f.nu > 0.0 && abs(f.delta) - across < H.leg_reach) {
                if (nq == H_QUADS) {
                    var kept = 0u;
                    for (var j = 0u; j < nq; j++) {
                        if (deltas[j] - across < H.leg_reach) {
                            quads[kept] = quads[j];
                            deltas[kept] = deltas[j];
                            kept++;
                        }
                    }
                    nq = kept;
                }
                if (nq < H_QUADS) {
                    quads[nq] = vec4f(f.z, f.nu, f.reach_t, f.grade_t);
                    deltas[nq] = abs(f.delta);
                    nq++;
                }
            }
            let ab = sb.p - sa.p;
            let l2 = dot(ab, ab);
            let g = select(0.0, clamp(dot(q - sa.p, ab) / l2, 0.0, 1.0), l2 > 0.0);
            let dist = length(q - sa.p - g * ab) * mpu;
            let support = (sa.h + g * (sb.h - sa.h)) + ln.side + ln.reach;
            let psi = h_phi((dist - least) / H.leg_reach) * h_phi(dist / support);
            if (psi <= 0.0) {
                continue;
            }
            let len_m = sqrt(l2) * mpu;
            let side = select(-1.0, 1.0, m_cross(ab, q - sa.p) <= 0.0);
            let z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, g) + (sa.bank + g * (sb.bank - sa.bank)) * side * dist;
            fb_w += psi;
            fb_z += psi * z;
            let lateral = -m_cross(h_unit(ab), q - sa.p) * mpu;
            let core = sa.h + g * (sb.h - sa.h) + ln.side;
            let rt = h_meet_terrain(sa, sb, h_left_share(lateral, dist), g, core, ln.reach);
            if (rt > H_UNKNOWN * 0.5) {
                fb_tw += psi;
                fb_t += psi * rt;
                fb_g += psi * h_terrain_grade(sa, sb, h_left_share(lateral, dist), g, core, ln.reach);
            }
        }
        let core_d = max(near, 0.0);
        if ((core_d >= max(feather, ln.reach) && !kept) || (nq == 0u && fb_w <= 0.0)) {
            continue;
        }
        // The line's surface: each healthy quad weighs its health times φ of its metres across past the
        // least over leg_reach, or the nearest quad alone; the fallback takes the rest of 1.
        var jn = 0u;
        for (var j = 1u; j < nq; j++) {
            if (deltas[j] < deltas[jn]) {
                jn = j;
            }
        }
        var sw = 0.0;
        var z = 0.0;
        var tw = 0.0;
        var tz = 0.0;
        var tg = 0.0;
        for (var j = 0u; j < nq; j++) {
            var a = quads[j].y * h_phi((deltas[j] - across) / H.leg_reach);
            if (H.legs_wall) {
                a = select(0.0, quads[j].y, j == jn);
            }
            sw += a;
            z += a * quads[j].x;
            if (quads[j].z > H_UNKNOWN * 0.5) {
                tw += a;
                tz += a * quads[j].z;
                tg += a * quads[j].w;
            }
        }
        let fb = max(1.0 - sw, 0.0);
        let zf = select(0.0, fb_z / fb_w, fb_w > 0.0);
        if (fb_tw > 0.0) {
            tw += fb;
            tz += fb * fb_t / fb_tw;
            tg += fb * fb_g / fb_tw;
        }
        h_insert(&hp, HRec(l, ln.rank, h_glob(block, ln), (z + fb * zf) / (sw + fb), core_d, H_BIG, feather, ln.reach, ln.ret, ln.cut, ln.fill, ln.fill_max, ln.cut_max, select(H_UNKNOWN, tz / tw, tw > 0.0), select(H_UNKNOWN, tg / tw, tw > 0.0)), keep);
    }
    // The fans: the core arc of a corner, and q's metres past its circle per pair of arms.
    var past: array<f32, H_PAIRS>;
    for (var k = 0u; k < H_PAIRS; k++) {
        past[k] = H_BIG;
    }
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
        // A point's fan relates its arms' ways; a round of the paved outline does not, and two legs of
        // one way relate by their heights only, as heights::present.
        let lines = block + mw(block + 6u);
        if (mw(cand) != M_FILL && l0 != M_NONE && l1 != M_NONE && mw(lines + M_LINE * l0) != mw(lines + M_LINE * l1)) {
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
                past[i0 * H_SLOTS + i1] = min(past[i0 * H_SLOTS + i1], length(v) * mpu - radius);
                past[i1 * H_SLOTS + i0] = past[i0 * H_SLOTS + i1];
            }
        }
        // The corner's outline: the arc of radius r − ring between the feet, and at a narrower foot
        // GP3's taper, as heights::present.
        let ring0 = mf(dk + 7u);
        let ring1 = mf(dk + 8u);
        let wide = max(ring0, ring1);
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
            if ((hp.recs[r].line == l0 || hp.recs[r].line == l1) && d < hp.recs[r].out) {
                hp.recs[r].out = d;
            }
        }
    }
    // A fan relates its two arms out to their feather past its circle; two lines that each meet a third
    // near q are related through it as far as that line's own feather reaches past both corners.
    for (var i = 0u; i < hp.n; i++) {
        for (var j = 0u; j < hp.n; j++) {
            if (i == j) {
                continue;
            }
            var w = h_via(past[i * H_SLOTS + j], max(hp.recs[i].feather, hp.recs[j].feather));
            for (var k = 0u; k < hp.n; k++) {
                if (k != i && k != j) {
                    w = max(w, h_via(max(past[i * H_SLOTS + k], past[k * H_SLOTS + j]), hp.recs[k].feather) * h_via(hp.recs[k].d, hp.recs[k].feather));
                }
            }
            hp.rel[i * H_SLOTS + j] = w;
        }
    }
    return hp;
}

// A relation at `past` metres that fades over via_fade before the feather, as heights::present.
fn h_via(past: f32, feather: f32) -> f32 {
    return h_phi((past - feather + H.via_fade) / H.via_fade);
}

// The pair's agreement, as heights::paved_as: a fan relates them, the face relates them as arms, or
// the rise between them is one that the lesser one's return grade climbs over the greater one's
// feather.
fn h_agree(hp: ptr<function, HPoint>, i: u32, j: u32, arms: u32) -> f32 {
    let upper = min(i, j);
    let lower = max(i, j);
    let wall = (*hp).recs[lower].ret * (*hp).recs[upper].feather / H.blend_steep;
    let ok = wall * 0.5;
    let dz = abs((*hp).recs[i].z - (*hp).recs[j].z);
    let face = f32(((arms >> i) & (arms >> j) & 1u) != 0u);
    return max(max(h_phi((dz - ok) / (wall - ok)), (*hp).rel[i * H_SLOTS + j]), face);
}

// The paved field for host o (H_SLOTS for none: a blend face), as heights::paved_as.
fn h_paved_as(hp: ptr<function, HPoint>, o: u32, arms: u32) -> f32 {
    let n = (*hp).n;
    var beta: array<f32, H_SLOTS>;
    for (var k = 0u; k < n; k++) {
        beta[k] = h_phi((*hp).recs[k].d / (*hp).recs[k].feather);
    }
    var sw = 0.0;
    var swz = 0.0;
    var near = 0u;
    for (var j = 0u; j < n; j++) {
        if ((*hp).recs[j].d < (*hp).recs[near].d) {
            near = j;
        }
        var w = beta[j];
        if (j != o && o < n) {
            w *= h_agree(hp, o, j, arms);
        }
        for (var i = 0u; i < n; i++) {
            if (i < j) {
                w *= select(1.0 - beta[i], 1.0 - beta[i] * h_agree(hp, i, j, arms), j == o);
            } else if (i > j && j != o) {
                w *= 1.0 - beta[i] * (1.0 - h_agree(hp, i, j, arms));
            }
        }
        sw += w;
        swz += w * (*hp).recs[j].z;
    }
    if (sw > 0.0) {
        return swz / sw;
    }
    return select((*hp).recs[near].z, (*hp).recs[o].z, o < n);
}

fn h_cone(s: f32, d: f32) -> f32 {
    if (d < H.round) {
        return s * d * d / (2.0 * H.round);
    }
    return s * (d - H.round * 0.5);
}

// The fill and cut at d metres past the outline of a road edge at `base` and their slopes, as
// heights::cones: (fill, cut, fill slope, cut slope).
fn h_cones(base: f32, reach_t: f32, d: f32, r: HRec) -> vec4f {
    let run = max(r.reach * (1.0 - H.fade_share) - H.round * 0.5, 1e-3);
    let known = reach_t > H_UNKNOWN * 0.5;
    var s = vec2f(r.fill, r.cut);
    var wf = 0.0;
    var wc = 0.0;
    if (known && base - reach_t > r.fill * run) {
        s.x = min((base - reach_t) / run, max(r.fill_max, r.fill));
        wf = max(base - reach_t - s.x * run, 0.0);
    }
    if (known && reach_t - base > r.cut * run) {
        s.y = min((reach_t - base) / run, max(r.cut_max, r.cut));
        wc = max(reach_t - base - s.y * run, 0.0);
    }
    return vec4f(base - wf - h_cone(s.x, d), base + wc + h_cone(s.y, d), s.x, s.y);
}

// The toe and crest round's width in height, as heights::span_k.
fn h_span_k(s: vec2f, grade: f32) -> f32 {
    if (grade <= H_UNKNOWN * 0.5) {
        return H_BIG;
    }
    return 0.5 * H.toe_span * max(min(abs(grade + s.x), abs(s.y - grade)), H.toe_gap_min);
}

// How far a record's cone has let go at q, and by how much it has moved, as heights::release.
fn h_release(r: HRec) -> vec2f {
    let w = r.reach * H.fade_share;
    let fade = h_smooth((r.d - (r.reach - w)) / w);
    return vec2f(fade, H.fade_gap * fade / max(1.0 - fade, 1e-6));
}

// The slope field at q, as heights::slope.
fn h_slope(block: u32, q: vec2f, mpu: f32, hp: ptr<function, HPoint>, t: f32) -> f32 {
    var lo = -H_BIG;
    var hi = H_BIG;
    var toe = H.toe_max;
    var seen = false;
    for (var k = 0u; k < (*hp).n; k++) {
        let r = (*hp).recs[k];
        if (r.reach <= 0.0) {
            continue;
        }
        seen = true;
        let d = min(r.d, r.out);
        let base = h_paved_as(hp, k, 0u);
        let fr = h_release(r);
        let fade = fr.x;
        let relax = fr.y;
        let c = h_cones(base, r.reach_t, d, r);
        let kq = h_soft_min(h_toe_width(d, h_span_k(c.zw, r.reach_g)), c.y - c.x);
        toe = min(toe, kq + fade * (H.toe_max - kq));
        lo = max(lo, c.x - relax);
        hi = min(hi, c.y + relax);
    }
    if (!seen) {
        return t;
    }
    let edge = h_slope_edge(block, q, mpu);
    let k = toe * h_smooth(edge / M_MARGIN);
    return t + h_smooth(edge / H.band_fade) * (h_smin(hi, h_smax(t, lo, k), k) - t);
}

// The records that region `id` relates as its junction's arms, a bit each, as heights::arms_of.
fn h_arms_of(block: u32, hp: ptr<function, HPoint>, id: u32) -> u32 {
    let lw = m_label(block, id);
    var arms = 0u;
    for (var k = 0u; k < (*hp).n; k++) {
        let line = (*hp).recs[k].line;
        var named = line == mw(lw + 2u);
        for (var w = 4u; w < 12u; w++) {
            named = named || m_arm_line(mw(lw + w)) == line;
        }
        arms |= select(0u, 1u << k, named);
    }
    return arms;
}

// A junction's region at q: its own painter, faded near each of its edges to the face beyond, as
// heights::region. The groups: a host beyond at its record's index, then the regions beyond (the
// nearest H_REGIONS), then the arms' cone base.
fn h_region(block: u32, q: vec2f, mpu: f32, hp: ptr<function, HPoint>, label: u32, arms: u32, t: f32) -> f32 {
    let n = (*hp).n;
    let own = h_paved_as(hp, H_SLOTS, arms);
    var dist: array<f32, H_SLOTS>;
    for (var k = 0u; k < H_SLOTS; k++) {
        dist[k] = H_BIG;
    }
    var outer = H_BIG;
    var beyond: array<vec2f, H_REGIONS>;
    var beyond_id: array<u32, H_REGIONS>;
    for (var k = 0u; k < H_REGIONS; k++) {
        beyond[k] = vec2f(H_BIG, 0.0);
        beyond_id[k] = M_NONE;
    }
    let head = m_bin_head(block, q);
    if (head != 0u) {
        var at = h_heights_start(block, q);
        let hc = mw(head + 2u);
        at += (hc >> 16u) + (hc & 0xffffu);
        for (var e = 0u; e < (mw(head + 3u) & 0xffffu); e++) {
            let ew = block + mw(block + 11u) + M_BLEND * mw(at + e);
            if (mw(ew + 4u) != label) {
                continue;
            }
            let d = h_point_segment(vec2f(mi(ew), mi(ew + 1u)), vec2f(mi(ew + 2u), mi(ew + 3u)), q) * mpu;
            if (d >= H.region_fade) {
                continue;
            }
            let other = mw(ew + 5u);
            let rule = select(0u, mw(m_label(block, other)) >> 24u, other != M_NONE);
            if (rule == 3u) {
                var k = H_REGIONS - 1u;
                for (var j = 0u; j < H_REGIONS; j++) {
                    if (beyond_id[j] == other) {
                        k = j;
                    }
                }
                if (d < beyond[k].x) {
                    beyond[k].x = d;
                    beyond_id[k] = other;
                    for (var j = H_REGIONS - 1u; j > 0u; j--) {
                        if (beyond[j].x < beyond[j - 1u].x) {
                            let t = beyond[j];
                            beyond[j] = beyond[j - 1u];
                            beyond[j - 1u] = t;
                            let i = beyond_id[j];
                            beyond_id[j] = beyond_id[j - 1u];
                            beyond_id[j - 1u] = i;
                        }
                    }
                }
                continue;
            }
            // A paved face beyond: its host line, if it has one; a slope or the terrain: the arms'
            // cone base.
            if (rule != 2u && rule != 4u) {
                outer = min(outer, d);
                continue;
            }
            let host = mw(m_label(block, other) + 2u);
            for (var k = 0u; k < n; k++) {
                if ((*hp).recs[k].line == host) {
                    dist[k] = min(dist[k], d);
                }
            }
        }
    }
    var sw = 1.0;
    var sv = 0.0;
    // The hosts beyond: σ is the host's presence times the region's agreement with it.
    for (var g = 0u; g < n; g++) {
        if (dist[g] >= H.region_fade) {
            continue;
        }
        var agreed = 0.0;
        for (var k = 0u; k < n; k++) {
            if (((arms >> k) & 1u) != 0u) {
                agreed = max(agreed, h_phi((*hp).recs[k].d / (*hp).recs[k].feather) * h_agree(hp, k, g, arms));
            }
        }
        let sigma = h_phi((*hp).recs[g].d / (*hp).recs[g].feather) * agreed;
        if (sigma <= 0.0) {
            continue;
        }
        let v = h_paved_as(hp, g, 0u);
        let s = h_smooth(dist[g] / H.region_fade);
        if (s <= 0.0) {
            return v;
        }
        let w = sigma * (1.0 - s) / s;
        sw += w;
        sv += w * (v - own);
    }
    // The regions beyond, nearest first (equal distances by their target): both regions' arms related.
    var vr: array<f32, H_REGIONS>;
    for (var k = 0u; k < H_REGIONS; k++) {
        if (beyond_id[k] != M_NONE) {
            vr[k] = h_paved_as(hp, H_SLOTS, arms | h_arms_of(block, hp, beyond_id[k]));
        }
    }
    for (var j = 1u; j < H_REGIONS; j++) {
        if (beyond_id[j] != M_NONE && beyond[j].x == beyond[j - 1u].x && vr[j] < vr[j - 1u]) {
            let t = vr[j];
            vr[j] = vr[j - 1u];
            vr[j - 1u] = t;
        }
    }
    for (var k = 0u; k < H_REGIONS; k++) {
        if (beyond_id[k] == M_NONE) {
            continue;
        }
        let s = h_smooth(beyond[k].x / H.region_fade);
        if (s <= 0.0) {
            return vr[k];
        }
        let w = (1.0 - s) / s;
        sw += w;
        sv += w * (vr[k] - own);
    }
    // The arms' cones as the slope has them, without their no-room walls.
    if (outer < H.region_fade) {
        var lo = -H_BIG;
        var hi = H_BIG;
        var seen = false;
        for (var k = 0u; k < n; k++) {
            let r = (*hp).recs[k];
            if (((arms >> k) & 1u) == 0u || r.reach <= 0.0) {
                continue;
            }
            let base = h_paved_as(hp, k, 0u);
            let c = h_cones(base, r.reach_t, min(r.d, r.out), r);
            let relax = h_release(r).y;
            lo = max(lo, base - h_cone(c.z, min(r.d, r.out)) - relax);
            hi = min(hi, base + h_cone(c.w, min(r.d, r.out)) + relax);
            seen = true;
        }
        if (seen) {
            let v = min(hi, max(t, lo));
            let s = h_smooth(outer / H.region_fade);
            if (s <= 0.0) {
                return v;
            }
            let w = (1.0 - s) / s;
            sw += w;
            sv += w * (v - own);
        }
    }
    return own + sv / sw;
}

// The height at q (frame units from the cell's origin) of the block, with the label there (M_NONE for
// terrain), the plain terrain t, and the first word of the bin's height lists (`ModelPoint.heights`,
// or 0 to find it): (height, kind).
fn height_at(block: u32, q: vec2f, label: u32, t: f32, start: u32) -> vec2f {
    H_BASE = round(t);
    let h = height_relative(block, q, label, t - H_BASE, start);
    return vec2f(h.x + H_BASE, h.y);
}

// height_at with every height less H_BASE.
fn height_relative(block: u32, q: vec2f, label: u32, t: f32, start: u32) -> vec2f {
    if (label == M_NONE) {
        return vec2f(t, H_TERRAIN);
    }
    let lw = m_label(block, label);
    let rule = mw(lw) >> 24u;
    if (rule == 0u) {
        return vec2f(t, H_TERRAIN);
    }
    H = h_rules(block);
    let mpu = h_scale(block, q);
    let material = mw(lw + 2u);
    var hp = h_present(block, q, mpu, vec2u(select(M_NONE, material, rule >= 2u), M_NONE), start);
    if (rule == 1u) {
        return vec2f(h_slope(block, q, mpu, &hp, t), H_SLOPE);
    }
    // A blend face (a junction's region) has no host, and it relates the junction's arms (its
    // material, host and guest lines), as heights::by_label.
    if (rule == 3u && hp.n > 0u) {
        return vec2f(h_region(block, q, mpu, &hp, label, h_arms_of(block, &hp, label), t), H_PAVED);
    }
    // The host: the label's material line; with none, the first record whose core holds q, else the
    // nearest, as heights::host.
    var o = H_SLOTS;
    for (var k = 0u; k < hp.n; k++) {
        if (hp.recs[k].line == material) {
            o = k;
        }
    }
    for (var k = 0u; k < hp.n && o == H_SLOTS && material == M_NONE; k++) {
        if (hp.recs[k].d <= 0.0) {
            o = k;
        }
    }
    if (o == H_SLOTS && material == M_NONE) {
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
    return vec2f(h_paved_as(&hp, o, 0u), H_PAVED);
}
