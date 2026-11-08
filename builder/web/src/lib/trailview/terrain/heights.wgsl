// The heights of the one road model: the crate's `faces::heights::height_at`, line for line, in f32.
// Per point, each line near it gives its own surface, its distance past its core and a presence; the
// paved field is a normalized painter of these surfaces in which each pair of records blends as far as
// they agree and the greater one rules inside its core. Slopes start from the face plan's paved outline
// (the records' outline edges): per host line a fill and a cut over its edges, released at the reach,
// and the ground is a smooth min(cut, max(T, fill)), where a line's cut rises to the bank of the other
// lines' fills, a wall class's up to its highest wall and then at its steepest cut.
// #include records.wgsl

const H_SLOTS: u32 = 10u;
// The pairs of H_SLOTS records.
const H_PAIRS: u32 = 45u;
const H_QUADS: u32 = 4u;
// The most regions beyond a region's edges that weigh at a point, as heights::REGIONS.
const H_REGIONS: u32 = 2u;
// The heights' rule words follow the block's head (heights::RULES of the crate).
const H_RULES_AT: u32 = 15u;
// The words of an outline edge, and its grade's steps per metre (records::EDGE, GRADE_STEPS).
const H_EDGE: u32 = 8u;
const H_GRADE_STEPS: f32 = 4096.0;
// An outline edge's cone reaches this many frame units past its line into its paved face before it falls
// on at face_max (heights::SIDE_UNITS).
const H_SIDE_UNITS: f32 = 32.0;
// HRec.rank and HLine.rank: the class rank below H_FLAGS, the line's flags from it on (a wall class, a
// rail, line water).
const H_FLAGS: u32 = 1u << 24u;
const H_WALL: u32 = 1u << 31u;
const H_RAIL: u32 = 1u << 30u;
const H_WATER: u32 = 1u << 29u;

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
    budget_round: f32,
    wall_max: f32,
    face_max: f32,
    region_fade: f32,
    legs_wall: bool,
}
var<private> H: HRules;
// The whole metres that height_at takes off every absolute height and adds back at the end: f32 sums of
// heights near the plain terrain keep their sub-millimetre steps on high ground.
var<private> H_BASE: f32;

fn h_rules(block: u32) -> HRules {
    let r = block + H_RULES_AT;
    return HRules(mf(r), mf(r + 1u), mf(r + 2u), mf(r + 3u), mf(r + 4u), mf(r + 5u), mf(r + 6u), mf(r + 7u), mf(r + 8u), mf(r + 9u), mf(r + 10u), mf(r + 11u), mf(r + 12u), mf(r + 13u), mf(r + 14u), mf(r + 15u), mf(r + 16u), mf(r + 17u), mf(r + 18u), mf(r + 20u), mw(r + 21u) == 1u);
}

// An unknown terrain where an outline edge's cones let go, or an unknown grade.
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
    // The fill cone starts this far over its edge and this far past it (water's dyke).
    rise: f32,
    crest: f32,
}

fn h_line(block: u32, l: u32) -> HLine {
    let at = block + mw(block + 6u) + M_LINE * l;
    let f = mw(at + 1u);
    let rank = mw(at + 7u) | select(0u, H_WALL, (f & M_WALL) != 0u) | select(0u, H_RAIL, (f & M_RAIL) != 0u) | select(0u, H_WATER, (f & M_WATER) != 0u);
    return HLine(mw(at + 2u), mw(at + 3u), mf(at + 4u), mf(at + 5u), mf(at + 6u), rank, mf(at + 8u) + mf(at + 9u), mf(at + 10u), mf(at + 14u), mf(at + 15u), mf(at + 16u), mf(at + 17u));
}

// Whether line l of the block is water; false for none.
fn h_line_water(block: u32, l: u32) -> bool {
    return (m_line_finish(block, l) & M_WATER) != 0u;
}

struct HSample {
    p: vec2f,
    h: f32,
    z: f32,
    bank: f32,
    grade: f32,
}

// A height word less H_BASE, H_UNKNOWN where it is unknown.
fn h_known_word(i: u32) -> f32 {
    let w = mw(i);
    return select(bitcast<f32>(w) - H_BASE, H_UNKNOWN, (w & 0x7f800000u) == 0x7f800000u);
}

fn h_sample(block: u32, ln: HLine, i: u32, mpu: f32) -> HSample {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return HSample(vec2f(mi(at), mi(at + 1u)), mf(at + 2u) * mpu, mf(at + 5u) - H_BASE, mf(at + 6u), mf(at + 7u));
}

// The mitre direction at sample i and the health radius on a side (+1 left, −1 right).
fn h_mitre(block: u32, ln: HLine, i: u32) -> vec2f {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return vec2f(mf(at + 8u), mf(at + 9u));
}

fn h_rho(block: u32, ln: HLine, i: u32, side: f32) -> f32 {
    let at = block + mw(block + 7u) + M_SAMPLE * (ln.first + i);
    return mf(at + select(11u, 10u, side > 0.0));
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
}

// Segment i of a line at q: in its mitre quad, or past the line's first or last sample, as
// heights::segment_of.
fn h_segment(block: u32, ln: HLine, i: u32, sa: HSample, sb: HSample, q: vec2f, mpu: f32) -> HFrame {
    var f = HFrame(false, 0.0, 0.0, 0.0);
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
        f.delta = h_across(q, sa.p, ab, ta, tb, t) * mpu;
        let rl = mpu / ((1.0 - t) / h_rho(block, ln, i, 1.0) + t / h_rho(block, ln, i + 1u, 1.0));
        let rr = mpu / ((1.0 - t) / h_rho(block, ln, i, -1.0) + t / h_rho(block, ln, i + 1u, -1.0));
        f.nu = h_health(rl, f.delta) * h_health(rr, -f.delta);
    } else if (first && da < 0.0 && db >= 0.0) {
        let past = -dot(q - sa.p, u) * mpu;
        t = 0.0;
        f.z = sa.z - sa.grade * past;
        f.delta = lateral;
        f.nu = h_health(h_rho(block, ln, i, 1.0) * mpu, lateral) * h_health(h_rho(block, ln, i, -1.0) * mpu, -lateral);
    } else if (last && db < 0.0 && da >= 0.0) {
        let past = dot(q - sb.p, u) * mpu;
        t = 1.0;
        f.z = sb.z + sb.grade * past;
        f.delta = lateral;
        f.nu = h_health(h_rho(block, ln, i + 1u, 1.0) * mpu, lateral) * h_health(h_rho(block, ln, i + 1u, -1.0) * mpu, -lateral);
    } else {
        return f;
    }
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
    feather: f32,
    ret: f32,
}

struct HPoint {
    recs: array<HRec, H_SLOTS>,
    n: u32,
    // rel[h_pair(i, j)]: how much a fan relates records i and j at the point; 0 where no fan does.
    rel: array<f32, H_PAIRS>,
}

// The index of the pair of records i and j (i != j) in a triangle of H_PAIRS.
fn h_pair(i: u32, j: u32) -> u32 {
    let a = min(i, j);
    let b = max(i, j);
    return b * (b - 1u) / 2u + a;
}

// Whether record a goes before record b in the painter's order: class rank, then the global key.
fn h_before(a: HRec, b: HRec) -> bool {
    let ra = a.rank % H_FLAGS;
    let rb = b.rank % H_FLAGS;
    return ra < rb || (ra == rb && (a.glob < b.glob || (a.glob == b.glob && a.line < b.line)));
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

// The lines present at q, as heights::present, into hp, a point as WGSL declares it (all zero). keep: the
// material lines of the labels asked for, never skipped as absent.
fn h_present(hp: ptr<function, HPoint>, block: u32, q: vec2f, mpu: f32, keep: vec2u, start: u32) {
    (*hp).n = 0u;
    let head = m_bin_head(block, q);
    if (head == 0u) {
        return;
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
        // The healthy quads: (own height, health).
        var quads: array<vec2f, H_QUADS>;
        var deltas: array<f32, H_QUADS>;
        var nq = 0u;
        // The least frame metres across of the line's quads that hold q so far, healthy or not.
        var across = H_BIG;
        // The fallback surface: a soft nearest of the segments' surfaces at their nearest points.
        var fb_w = 0.0;
        var fb_z = 0.0;
        // A kept line past every segment's support (a ford's water under its wet face): its surface at the
        // nearest segment, as heights::present.
        var nearest = H_BIG;
        var nearest_z = 0.0;
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
                    quads[nq] = vec2f(f.z, f.nu);
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
            let len_m = sqrt(l2) * mpu;
            let side = select(-1.0, 1.0, m_cross(ab, q - sa.p) <= 0.0);
            let z = h_hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, g) + (sa.bank + g * (sb.bank - sa.bank)) * side * dist;
            if (dist < nearest) {
                nearest = dist;
                nearest_z = z;
            }
            if (psi <= 0.0) {
                continue;
            }
            fb_w += psi;
            fb_z += psi * z;
        }
        let core_d = max(near, 0.0);
        if ((core_d >= max(feather, ln.reach) && !kept) || (nq == 0u && fb_w <= 0.0 && !kept)) {
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
        for (var j = 0u; j < nq; j++) {
            var a = quads[j].y * h_phi((deltas[j] - across) / H.leg_reach);
            if (H.legs_wall) {
                a = select(0.0, quads[j].y, j == jn);
            }
            sw += a;
            z += a * quads[j].x;
        }
        let fb = max(1.0 - sw, 0.0);
        let zf = select(nearest_z, fb_z / fb_w, fb_w > 0.0);
        h_insert(hp, HRec(l, ln.rank, h_glob(block, ln), (z + fb * zf) / (sw + fb), core_d, feather, ln.ret), keep);
    }
    // The fans: q's metres past a fan's circle per pair of its arms. A fan relates its two arms out to their
    // feather past its circle; two lines that each meet a third near q are related through it as far as that
    // line's own feather reaches past both corners. With no fan every relation is 0, as rel holds.
    if (nd == 0u) {
        return;
    }
    var past: array<f32, H_PAIRS>;
    for (var k = 0u; k < H_PAIRS; k++) {
        past[k] = H_BIG;
    }
    var fans = false;
    let extras = m_extras(block);
    let lines = block + mw(block + 6u);
    for (var e = 0u; e < nd; e++) {
        let item = mw(ds + e);
        let cand = m_cand(block, item & 0xffffu);
        let dk = extras + mw(cand + 5u) + M_DISC * (item >> 16u);
        let v = q - vec2f(mi(dk), mi(dk + 1u));
        let radius = mf(dk + 2u) * mpu;
        let l0 = mw(dk + 3u);
        let l1 = mw(dk + 4u);
        // A point's fan relates its arms' ways; a round of the paved outline does not, and two legs of
        // one way relate by their heights only, as heights::present.
        if (mw(cand) != M_FILL && l0 != M_NONE && l1 != M_NONE && mw(lines + M_LINE * l0) != mw(lines + M_LINE * l1)) {
            var i0 = H_SLOTS;
            var i1 = H_SLOTS;
            for (var r = 0u; r < (*hp).n; r++) {
                if ((*hp).recs[r].line == l0) {
                    i0 = r;
                }
                if ((*hp).recs[r].line == l1) {
                    i1 = r;
                }
            }
            if (i0 < H_SLOTS && i1 < H_SLOTS) {
                let p = h_pair(i0, i1);
                past[p] = min(past[p], length(v) * mpu - radius);
                fans = true;
            }
        }
    }
    if (!fans) {
        return;
    }
    for (var i = 0u; i < (*hp).n; i++) {
        for (var j = i + 1u; j < (*hp).n; j++) {
            var w = h_via(past[h_pair(i, j)], max((*hp).recs[i].feather, (*hp).recs[j].feather));
            for (var k = 0u; k < (*hp).n; k++) {
                if (k != i && k != j) {
                    w = max(w, h_via(max(past[h_pair(i, k)], past[h_pair(k, j)]), (*hp).recs[k].feather) * h_via((*hp).recs[k].d, (*hp).recs[k].feather));
                }
            }
            (*hp).rel[h_pair(i, j)] = w;
        }
    }
}

// A relation at `past` metres that fades over via_fade before the feather, as heights::present.
fn h_via(past: f32, feather: f32) -> f32 {
    return h_phi((past - feather + H.via_fade) / H.via_fade);
}

// The pair's agreement, as heights::paved_as: a fan relates them, the face relates them as arms, or
// the rise between them is one that the lesser one's return grade climbs over the greater one's
// feather. A return grade of 0 (water's) climbs no rise.
fn h_agree(hp: ptr<function, HPoint>, i: u32, j: u32, arms: u32) -> f32 {
    let upper = min(i, j);
    let lower = max(i, j);
    let wall = (*hp).recs[lower].ret * (*hp).recs[upper].feather / H.blend_steep;
    let ok = wall * 0.5;
    let dz = abs((*hp).recs[i].z - (*hp).recs[j].z);
    let face = f32(((arms >> i) & (arms >> j) & 1u) != 0u);
    var rel = 0.0;
    if (i != j) {
        rel = (*hp).rel[h_pair(i, j)];
    }
    let rise = select(0.0, h_phi((dz - ok) / (wall - ok)), wall > 0.0);
    return max(max(rise, rel), face);
}

// Whether record k is of the painter's kind: water, or a way.
fn h_kind(hp: ptr<function, HPoint>, k: u32, water: bool) -> bool {
    return (((*hp).recs[k].rank & H_WATER) != 0u) == water;
}

// The paved field for host o (H_SLOTS for none: a blend face), as heights::paved_as: of the records of the
// painter's kind (`water`) only.
fn h_paved_as(hp: ptr<function, HPoint>, o: u32, arms: u32, water: bool) -> f32 {
    let n = (*hp).n;
    var beta: array<f32, H_SLOTS>;
    for (var k = 0u; k < n; k++) {
        beta[k] = h_phi((*hp).recs[k].d / (*hp).recs[k].feather);
    }
    var sw = 0.0;
    var swz = 0.0;
    var near = H_SLOTS;
    for (var j = 0u; j < n; j++) {
        if (!h_kind(hp, j, water)) {
            continue;
        }
        if (near == H_SLOTS || (*hp).recs[j].d < (*hp).recs[near].d) {
            near = j;
        }
        var w = beta[j];
        if (j != o && o < n) {
            w *= h_agree(hp, o, j, arms);
        }
        for (var i = 0u; i < n; i++) {
            if (!h_kind(hp, i, water)) {
                continue;
            }
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
    if (o < n) {
        return (*hp).recs[o].z;
    }
    return select(0.0, (*hp).recs[min(near, H_SLOTS - 1u)].z, near < H_SLOTS);
}

fn h_cone(s: f32, d: f32) -> f32 {
    if (d < H.round) {
        return s * d * d / (2.0 * H.round);
    }
    return s * (d - H.round * 0.5);
}

// The no-room rule of a cone of slope s from an edge whose ends stand `need` above (a fill) or below
// (a cut) the plain terrain where the cone should meet it (`known`: which ends know it), with walls up
// to `wall` before the face steepens on as steep as the need asks, and `crest` metres less run, as
// heights::fit: (the slope, the wall at each end).
fn h_fit(s: f32, most: f32, need: vec2f, known: vec2<bool>, wall: f32, reach: f32, crest: f32) -> vec3f {
    let run = max(max(reach * (1.0 - H.fade_share) - H.round * 0.5, 1e-3) - crest, 1e-3);
    let top = max(select(-H_BIG, need.x, known.x), select(-H_BIG, need.y, known.y));
    let s0 = min(s, H.face_max);
    var sc = select(min(top / run, max(min(most, H.face_max), s0)), s0, top <= s0 * run);
    if (top - sc * run > wall) {
        sc = (top - wall) / run;
    }
    let w = select(vec2f(0.0), clamp(need - sc * run, vec2f(0.0), vec2f(wall)), known);
    return vec3f(sc, w);
}

// The greatest of top(x) − c |q − x| over an edge whose top runs linearly (sign 1, a fill), or the least
// of top(x) + c |q − x| (sign −1, a cut), with c = √(s² + g²) for the edge's grade g, as
// heights::edge_cone: q lies `along` metres along the edge and `off` metres off its line. With a crest b
// the cone measures max(0, |q − x| − b).
fn h_edge_cone(along: f32, off: f32, len: f32, top: vec2f, s: f32, sign: f32, crest: f32) -> f32 {
    let g = select(0.0, (top.y - top.x) / len, len > 0.0);
    let c = sqrt(s * s + g * g);
    var u = along + sign * sign_of(g) * sqrt(max(crest * crest - off * off, 0.0));
    if (off * c > crest * s) {
        u = along + sign * off * g / s;
    }
    u = clamp(u, 0.0, len);
    let d = sqrt((u - along) * (u - along) + off * off);
    return top.x + g * u - sign * c * max(d - crest, 0.0);
}

// The sign of v with 1 for 0, as Rust's signum.
fn sign_of(v: f32) -> f32 {
    return select(-1.0, 1.0, v >= 0.0);
}

// How much the round of a cone of slope s at its foot holds it back at d metres, as heights::round_back.
fn h_round_back(s: f32, d: f32) -> f32 {
    return s * d - h_cone(s, d);
}

// The toe and crest round's width in height, as heights::span_k.
fn h_span_k(sf: f32, sc: f32, grade: f32) -> f32 {
    if (grade <= H_UNKNOWN * 0.5) {
        return H_BIG;
    }
    return 0.5 * H.toe_span * max(min(abs(grade + sf), abs(sc - grade)), H.toe_gap_min);
}

// How far a cone has let go at d metres from its edge, and by how much it has moved, as heights::release.
fn h_release(d: f32, reach: f32) -> vec2f {
    let w = reach * H.fade_share;
    let fade = h_smooth((d - (reach - w)) / w);
    return vec2f(fade, H.fade_gap * fade / max(1.0 - fade, 1e-6));
}

// The soft greatest of the first n fills whose bit is set in `counted`, as heights::soft_top: the mean of
// the two greatest, the second weighted by (1 − gap / width)², the width growing from 0 at the greatest
// one's outline.
fn h_soft_top(fills: ptr<function, array<f32, H_SLOTS>>, dists: ptr<function, array<f32, H_SLOTS>>, n: u32, counted: u32) -> f32 {
    var top = H_SLOTS;
    for (var j = 0u; j < n; j++) {
        if (((counted >> j) & 1u) != 0u && (top == H_SLOTS || (*fills)[j] > (*fills)[top])) {
            top = j;
        }
    }
    if (top == H_SLOTS) {
        return -H_BIG;
    }
    var second = -H_BIG;
    for (var j = 0u; j < n; j++) {
        if (((counted >> j) & 1u) != 0u && j != top) {
            second = max(second, (*fills)[j]);
        }
    }
    let v = (*fills)[top];
    let width = h_toe_width((*dists)[top], H.budget_round);
    if (width <= 0.0 || second <= -H_BIG * 0.5 || v <= -H_BIG * 0.5) {
        return v;
    }
    let x = max(1.0 - (v - second) / width, 0.0);
    return (v + x * x * second) / (1.0 + x * x);
}

// One host line's outline near q, as heights::Group: its released fill and cut, the cut it may rise to
// toward its budget, the metres to its nearest edge, its least toe width and how far it has let go.
struct HGroup {
    line: u32,
    rank: u32,
    glob: u32,
    fill: f32,
    // Its fill without a rise and a crest, which bounds its own cut.
    plain: f32,
    cut: f32,
    steep: f32,
    d: f32,
    toe: f32,
    fade: f32,
}

struct HGroups {
    g: array<HGroup, H_SLOTS>,
    n: u32,
}

// Whether group a goes before group b in the painter's order: class rank, then the global key, then the
// line.
fn h_group_before(a: HGroup, b: HGroup) -> bool {
    let ra = a.rank % H_FLAGS;
    let rb = b.rank % H_FLAGS;
    return ra < rb || (ra == rb && (a.glob < b.glob || (a.glob == b.glob && a.line < b.line)));
}

// A line's group ends, as heights::groups: its round at the foot taken once at its distance, then in
// the painter's order among the first H_SLOTS.
fn h_close(gs: ptr<function, HGroups>, g0: HGroup, ln: HLine) {
    if (g0.d >= H_BIG * 0.5) {
        return;
    }
    var g = g0;
    g.fade = h_release(g.d, ln.reach).x;
    g.plain += h_round_back(ln.fill, g.d);
    g.fill += h_round_back(ln.fill, max(g.d - ln.crest, 0.0));
    g.cut -= h_round_back(ln.cut, g.d);
    g.steep -= h_round_back(min(max(ln.cut_max, ln.cut), H.face_max), g.d);
    if ((*gs).n == H_SLOTS) {
        if (!h_group_before(g, (*gs).g[H_SLOTS - 1u])) {
            return;
        }
        (*gs).n--;
    }
    var j = (*gs).n;
    loop {
        if (j == 0u || !h_group_before(g, (*gs).g[j - 1u])) {
            break;
        }
        (*gs).g[j] = (*gs).g[j - 1u];
        j--;
    }
    (*gs).g[j] = g;
    (*gs).n++;
}

// The host lines of the outline edges that q's bin lists, as heights::groups. A bin lists its edges by
// host line, in half words after their count; a block with no outline edges lists none. `dry` (a zone) leaves
// out the hosts of line water.
fn h_groups(block: u32, q: vec2f, mpu: f32, dry: bool) -> HGroups {
    var gs: HGroups;
    gs.n = 0u;
    let head = m_bin_head(block, q);
    if (head == 0u || mw(block + 14u) >= mw(block + 9u)) {
        return gs;
    }
    let hc = mw(head + 2u);
    let at = h_heights_start(block, q) + (hc >> 16u) + (hc & 0xffffu) + (mw(head + 3u) & 0xffffu);
    let n = mw(at) & 0xffffu;
    let edges = block + mw(block + 14u);
    var g = HGroup(M_NONE, 0u, 0u, -H_BIG, -H_BIG, H_BIG, H_BIG, H_BIG, H_BIG, 1.0);
    var ln: HLine;
    for (var k = 1u; k <= n; k++) {
        let e = edges + H_EDGE * ((mw(at + k / 2u) >> (16u * (k % 2u))) & 0xffffu);
        let line = mw(e + 3u) & 0xffffu;
        if (dry && h_line_water(block, line)) {
            continue;
        }
        if (line != g.line) {
            if (g.line != M_NONE) {
                h_close(&gs, g, ln);
            }
            ln = h_line(block, line);
            g = HGroup(line, ln.rank, h_glob(block, ln), -H_BIG, -H_BIG, H_BIG, H_BIG, H_BIG, H_BIG, 1.0);
        }
        // Metres along and off the edge from q − a; b is two i16 offsets from a.
        let a = vec2f(mi(e), mi(e + 1u));
        let ob = mw(e + 2u);
        let ab = vec2f(f32(bitcast<i32>(ob << 16u) >> 16u), f32(bitcast<i32>(ob) >> 16u));
        let v = q - a;
        let lu = length(ab);
        let u = select(vec2f(1.0, 0.0), ab / lu, lu > 0.0);
        let along = dot(v, u) * mpu;
        let side = -m_cross(u, v);
        let off = abs(side) * mpu;
        let len = lu * mpu;
        let foot = clamp(along, 0.0, len) - along;
        let d = sqrt(foot * foot + off * off);
        if (d >= ln.reach) {
            continue;
        }
        let h = vec2f(mf(e + 4u), mf(e + 5u)) - H_BASE;
        let t = vec2f(h_known_word(e + 6u), h_known_word(e + 7u));
        let known = t > vec2f(H_UNKNOWN * 0.5);
        let gq = bitcast<i32>(mw(e + 3u)) >> 16u;
        let grade = select(f32(gq) / H_GRADE_STEPS, H_UNKNOWN, gq == -32768);
        let fr = h_release(d, ln.reach);
        let wall = select(0.0, H.wall_max, (ln.rank & H_WALL) != 0u);
        // The fill starts ln.rise over the edge and ln.crest past it.
        let f = h_fit(ln.fill, ln.fill_max, h + ln.rise - t, known, wall, ln.reach, ln.crest);
        let c = h_fit(ln.cut, ln.cut_max, t - h, known, wall, ln.reach, 0.0);
        g.d = min(g.d, d);
        let w = h_toe_width(d, h_span_k(f.x, c.x, grade));
        g.toe = min(g.toe, w + fr.x * (H.toe_max - w));
        // Past its line into its paved face (from H_SIDE_UNITS on) an edge's cone falls on at face_max,
        // as heights::groups.
        let inside = select(0.0, max(-side - H_SIDE_UNITS, 0.0) * mpu * H.face_max, lu > 0.0);
        let fill = h_edge_cone(along, off, len, h + ln.rise - f.yz, f.x, 1.0, ln.crest);
        var plain = fill;
        if (ln.rise > 0.0 || ln.crest > 0.0) {
            let pf = h_fit(ln.fill, ln.fill_max, h - t, known, wall, ln.reach, 0.0);
            plain = h_edge_cone(along, off, len, h - pf.yz, pf.x, 1.0, 0.0);
        }
        g.fill = max(g.fill, fill - fr.y - inside);
        g.plain = max(g.plain, plain - fr.y - inside);
        g.cut = min(g.cut, h_edge_cone(along, off, len, h + c.yz, c.x, -1.0, 0.0) + fr.y + inside);
        // The highest cut toward the budget: a wall class's wall up to wall_max at its edge, then a face
        // at the steepest cut from the wall's top.
        let steep = min(max(ln.cut_max, ln.cut), H.face_max);
        let top = h + wall;
        let s = h_fit(steep, steep, t - top, known, 0.0, ln.reach, 0.0);
        g.steep = min(g.steep, h_edge_cone(along, off, len, top + s.yz, s.x, -1.0, 0.0) + fr.y + inside);
    }
    if (g.line != M_NONE) {
        h_close(&gs, g, ln);
    }
    return gs;
}

// The slope field at q, as heights::slope: per host line of the outline near q its fill and cut, then a
// smooth min(cut, max(T, fill)). A line's cut rises toward its bank budget (the soft greatest fill of
// the other lines that are no rail and no water) up to its highest cut, in a round that grows from 0 at
// its own outline and at the outline of every line in the budget; where its own plain fill passes its
// cut, that fill bounds the cut.
fn h_slope(block: u32, q: vec2f, mpu: f32, t: f32, dry: bool) -> f32 {
    let gs = h_groups(block, q, mpu, dry);
    let n = gs.n;
    if (n == 0u) {
        return t;
    }
    var fills: array<f32, H_SLOTS>;
    var dists: array<f32, H_SLOTS>;
    var toe = H.toe_max;
    var roads = 0u;
    for (var k = 0u; k < n; k++) {
        let g = gs.g[k];
        let kq = h_soft_min(g.toe, g.cut - g.fill);
        toe = min(toe, kq + g.fade * (H.toe_max - kq));
        fills[k] = g.fill;
        dists[k] = g.d;
        roads |= select(1u << k, 0u, (g.rank & (H_RAIL | H_WATER)) != 0u);
    }
    let lo = h_soft_top(&fills, &dists, n, 0xffffffffu);
    var hi = H_BIG;
    for (var k = 0u; k < n; k++) {
        let g = gs.g[k];
        let counted = roads & ~(1u << k);
        let budget = h_soft_top(&fills, &dists, n, counted);
        var near = g.d;
        for (var j = 0u; j < n; j++) {
            if (((counted >> j) & 1u) != 0u) {
                near = min(near, dists[j]);
            }
        }
        let w = h_toe_width(near, H.budget_round);
        let lift = h_smin(budget, g.steep, w);
        hi = min(hi, max(h_smax(g.cut, lift, w), min(g.plain, g.steep)));
    }
    let edge = h_slope_edge(block, q, mpu);
    let k = toe * h_smooth(edge / M_MARGIN);
    return t + h_smooth(edge / H.band_fade) * (h_smin(hi, h_smax(t, lo, k), k) - t);
}

// The records that region `id` relates as its junction's arms, a bit each, as heights::arms_of.
fn h_arms_of(block: u32, hp: ptr<function, HPoint>, id: u32) -> u32 {
    let lw = m_label(block, id);
    let jr = m_junction(block, id);
    var arms = 0u;
    for (var k = 0u; k < (*hp).n; k++) {
        let line = (*hp).recs[k].line;
        var named = line == mw(lw + 2u);
        if (jr == M_NONE) {
            for (var w = 4u; w < 12u; w++) {
                named = named || mw(lw + w) == line;
            }
        } else {
            for (var a = 0u; a < (mw(jr + 2u) & 0xffu); a++) {
                named = named || (mw(m_arm(jr, a)) & 0xffffu) == line;
            }
        }
        arms |= select(0u, 1u << k, named);
    }
    return arms;
}

// A junction's region at q: its own painter, faded near each of its edges to the face beyond, as
// heights::region. The groups: a host beyond at its record's index, then the regions beyond (the
// nearest H_REGIONS). Toward a slope or the terrain the region keeps its own painter. `water`: the region's
// kind, its material line's.
fn h_region(block: u32, q: vec2f, mpu: f32, hp: ptr<function, HPoint>, label: u32, arms: u32, water: bool) -> f32 {
    let n = (*hp).n;
    let own = h_paved_as(hp, H_SLOTS, arms, water);
    var dist: array<f32, H_SLOTS>;
    for (var k = 0u; k < H_SLOTS; k++) {
        dist[k] = H_BIG;
    }
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
            // A paved face beyond: its host line, if it has one.
            if (rule != 2u && rule != 4u) {
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
        let v = h_paved_as(hp, g, 0u, ((*hp).recs[g].rank & H_WATER) != 0u);
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
            vr[k] = h_paved_as(hp, H_SLOTS, arms | h_arms_of(block, hp, beyond_id[k]), water);
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
    // A slope reads only the outline edges; a zone leaves out the water's.
    if (rule == 1u) {
        return vec2f(h_slope(block, q, mpu, t, (mw(lw) & 0xffu) == M_SURFACE_ZONE), H_SLOPE);
    }
    return h_paved_at(block, q, mpu, label, rule, mw(lw + 2u), t, start);
}

// height_relative of a paved label. Tint zeroes a function's vars at its entry: hp lives here, so only a paved
// texel pays for zeroing it.
fn h_paved_at(block: u32, q: vec2f, mpu: f32, label: u32, rule: u32, material: u32, t: f32, start: u32) -> vec2f {
    var hp: HPoint;
    h_present(&hp, block, q, mpu, vec2u(material, M_NONE), start);
    // A blend face (a junction's region) has no host, and it relates the junction's arms (its
    // material, host and guest lines), as heights::by_label; with no record of its kind, the plain terrain.
    let water = h_line_water(block, material);
    var kind = false;
    for (var k = 0u; k < hp.n; k++) {
        kind = kind || h_kind(&hp, k, water);
    }
    if (rule == 3u && kind) {
        return vec2f(h_region(block, q, mpu, &hp, label, h_arms_of(block, &hp, label), water), H_PAVED);
    }
    // The host: the label's material line; with none (a way's band face), the first way record whose core
    // holds q, else the nearest way record, as heights::host.
    var o = H_SLOTS;
    for (var k = 0u; k < hp.n; k++) {
        if (hp.recs[k].line == material) {
            o = k;
        }
    }
    for (var k = 0u; k < hp.n && o == H_SLOTS && material == M_NONE; k++) {
        if (hp.recs[k].d <= 0.0 && h_kind(&hp, k, false)) {
            o = k;
        }
    }
    if (o == H_SLOTS && material == M_NONE) {
        var best = H_BIG;
        for (var k = 0u; k < hp.n; k++) {
            if (hp.recs[k].d < best && h_kind(&hp, k, false)) {
                best = hp.recs[k].d;
                o = k;
            }
        }
    }
    if (o == H_SLOTS) {
        return vec2f(t, H_PAVED);
    }
    return vec2f(h_paved_as(&hp, o, 0u, (hp.recs[o].rank & H_WATER) != 0u), H_PAVED);
}
