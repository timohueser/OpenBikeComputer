// Spike (far=strokes): the ways of a far page drawn, not looked up. Each centre-line segment of a cell's
// record lines is one instance: a quad about it, and per fragment the capsule of the segment. The union of
// a line's capsules is its stroke with round joins and ends; the depth test is the overlay's "least key
// wins". Four passes into a 4 × 4 atlas of page tiles, which stamp.wgsl reads:
// - owner: the winning line, its surface and its frame (depth: key, then the distance);
// - distances: the least distance past each band's edge over all lines (blend min);
// - fill: the greatest fill cone (depth greater); cut: the least cut, raised to the fill below it (depth less).
// #include heights.wgsl

struct StrokeJob {
    // The page's corner in frame units from the cell's origin, and its tile's first pixel in the atlas.
    q0: vec2i,
    tile: vec2f,
    // Frame units per texel of the page's level.
    upt: f32,
    block: u32,
    // The count of key ranks of the cell, and the height at the middle of the cone passes' depth.
    keys: f32,
    base: f32,
    // The heights' rule words of the block (heights::RULES), as floats.
    rules: array<vec4f, 6>,
}

@group(0) @binding(0) var<uniform> sj: StrokeJob;
// The cut pass reads the fills.
@group(1) @binding(0) var lower: texture_2d<f32>;

// The pass whose radius the vertex stage draws: owner, distances, cones.
override PASS: u32 = 0u;
// As MASK_RANGE and PAINT_TEXELS of stamp.wgsl.
const S_MASK_RANGE: f32 = 12.75;
const S_PAINT_TEXELS: f32 = 2.0;
const S_BIG: f32 = 60000.0;
// Metres that the cone passes' depth spans about `base`.
const S_SPAN: f32 = 8192.0;
// Line end kinds, as records.rs.
const S_LEVEL: u32 = 2u;

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) inst: vec4u,
}

fn s_rules() -> HRules {
    let r = sj.rules;
    return HRules(r[0].x, r[0].y, r[0].z, r[0].w, r[1].x, r[1].y, r[1].z, r[1].w, r[2].x, r[2].y, r[2].z, r[2].w, r[3].x, r[3].y, r[3].z, r[3].w, r[4].x, r[4].y, r[4].z, r[5].x, bitcast<u32>(r[5].y) == 1u);
}

// The words of line l: its first word.
fn s_line_at(l: u32) -> u32 {
    return sj.block + mw(sj.block + 6u) + M_LINE * l;
}

@vertex
fn stroke_vertex(@builtin(vertex_index) v: u32, @location(0) inst: vec4u) -> VOut {
    let block = sj.block;
    let ln = h_line(block, inst.x);
    let at = s_line_at(inst.x);
    let wa = block + mw(block + 7u) + M_SAMPLE * (ln.first + (inst.y & 0xffffu));
    let a = vec2f(mi(wa), mi(wa + 1u));
    let b = vec2f(mi(wa + M_SAMPLE), mi(wa + M_SAMPLE + 1u));
    let mpu = h_scale(block, 0.5 * (a + b));
    let half = max(mf(wa + 2u), mf(wa + M_SAMPLE + 2u)) * mpu;
    let sh = mf(at + 8u);
    var r = half + max(sh, S_PAINT_TEXELS * sj.upt * mpu);
    if (PASS == 1u) {
        r = max(half + sh + mf(at + 9u) + ln.reach, half + S_MASK_RANGE);
    } else if (PASS == 2u) {
        r = half + sh + ln.reach;
    }
    let ru = r / mpu * 1.01 + sj.upt;
    let ab = b - a;
    let len = length(ab);
    let u = select(vec2f(1.0, 0.0), ab / len, len > 0.0);
    let n = vec2f(-u.y, u.x);
    let c = vec2f(f32(v & 1u), f32(v >> 1u)) * 2.0 - 1.0;
    let p = select(a - u * ru, b + u * ru, c.x > 0.0) + n * ru * c.y;
    let texel = (p - vec2f(sj.q0)) / sj.upt;
    var out: VOut;
    out.pos = vec4f(texel.x / 128.0 - 1.0, 1.0 - texel.y / 128.0, 0.5, 1.0);
    out.inst = inst;
    return out;
}

// A texel and the segment of an instance: the clamped share t of the nearest point and the metres to it.
struct Hit {
    q: vec2f,
    ln: HLine,
    line: u32,
    i: u32,
    sa: HSample,
    sb: HSample,
    wa: u32,
    u: vec2f,
    mpu: f32,
    t: f32,
    d: f32,
    // Metres across from the middle as model.wgsl signs them (sign of cross(u, q − a)).
    across: f32,
    // The half-width at t, the shoulder and the verge, metres.
    half: f32,
    sh: f32,
    verge: f32,
    // Whether the texel lies past a square end (a level change).
    cut: bool,
}

fn s_hit(pos: vec4f, inst: vec4u) -> Hit {
    var k: Hit;
    H = s_rules();
    let block = sj.block;
    let texel = floor(pos.xy - sj.tile);
    k.q = vec2f(sj.q0) + (texel + 0.5) * sj.upt;
    k.line = inst.x;
    k.i = inst.y & 0xffffu;
    k.ln = h_line(block, inst.x);
    k.mpu = h_scale(block, k.q);
    k.sa = h_sample(block, k.ln, k.i, k.mpu);
    k.sb = h_sample(block, k.ln, k.i + 1u, k.mpu);
    k.wa = block + mw(block + 7u) + M_SAMPLE * (k.ln.first + k.i);
    let ab = k.sb.p - k.sa.p;
    let l2 = max(dot(ab, ab), 1e-9);
    let tu = dot(k.q - k.sa.p, ab) / l2;
    k.t = clamp(tu, 0.0, 1.0);
    let v = k.q - k.sa.p - k.t * ab;
    k.d = length(v) * k.mpu;
    k.u = ab / sqrt(l2);
    k.across = sign(m_cross(ab, k.q - k.sa.p)) * k.d;
    k.half = mix(k.sa.h, k.sb.h, k.t);
    let at = s_line_at(inst.x);
    k.sh = mf(at + 8u);
    k.verge = mf(at + 9u);
    // Square ends at a level change and where the way goes on as its next leg (CUT_START, CUT_END of strokes.ts).
    let square = vec2<bool>(mw(at + 12u) == S_LEVEL || (inst.y & 0x10000u) != 0u, mw(at + 13u) == S_LEVEL || (inst.y & 0x20000u) != 0u);
    k.cut = (k.i == 0u && tu < 0.0 && square.x) || (k.i + 2u == k.ln.count && tu > 1.0 && square.y);
    return k;
}

// The owner line's own surface at the texel: its quad's (heights::segment_of), else its record surface.
fn s_surface(k: Hit) -> f32 {
    let f = h_segment(sj.block, k.ln, k.i, k.sa, k.sb, k.q, k.mpu);
    if (f.ok) {
        return f.z;
    }
    let s = h_surface(sj.block, k.ln, k.i, k.q, k.mpu);
    if (s.y > 0.0) {
        return s.x;
    }
    return mix(k.sa.z, k.sb.z, k.t);
}

struct OwnerOut {
    // The surface, metres across, along, the half-width.
    @location(0) frame: vec4f,
    // The line, and from bit 16 its part: 0 the carriageway, 1 the shoulder, 2 the paint's reach past it.
    @location(1) owner: u32,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn owner_fragment(in: VOut) -> OwnerOut {
    let k = s_hit(in.pos, in.inst);
    let reach = k.half + max(k.sh, S_PAINT_TEXELS * sj.upt * k.mpu);
    if (k.cut || k.d > reach) {
        discard;
    }
    let part = select(select(2u, 1u, k.d <= k.half + k.sh), 0u, k.d <= k.half);
    let key = f32(select(in.inst.z, in.inst.w, k.t >= 0.5));
    let dn = clamp(k.d / max(reach, 1e-3), 0.0, 0.999);
    var depth = (f32(part) * sj.keys + key + dn) / (3.0 * sj.keys);
    if (part == 2u) {
        depth = (2.0 + dn) / 3.0;
    }
    var out: OwnerOut;
    out.frame = vec4f(s_surface(k), k.across, mix(mf(k.wa + 3u), mf(k.wa + M_SAMPLE + 3u), k.t), k.half);
    out.owner = k.line | (part << 16u);
    out.depth = depth;
    return out;
}

// The distances past the carriageway's edge, the core's (shoulder and verge) and the slope band's; the
// blend keeps the least of each.
@fragment
fn distance_fragment(in: VOut) -> @location(0) vec4f {
    let k = s_hit(in.pos, in.inst);
    let core = k.half + k.sh + k.verge;
    let band = core + k.ln.reach;
    if (k.cut || k.d > max(band, k.half + S_MASK_RANGE)) {
        discard;
    }
    return vec4f(k.d - k.half, k.d - core, k.d - band, S_BIG);
}

// The shoulder's outer edge on one side (s = +1 where heights' metres across are positive): the segment
// between the edge points at both samples, the paved heights there, the plain terrain where the cones let go.
struct Edge {
    along: f32,
    off: f32,
    len: f32,
    d: f32,
    inside: f32,
    z: vec2f,
    t: vec2f,
    known: vec2<bool>,
}

fn s_edge(k: Hit, s: f32) -> Edge {
    var e: Edge;
    let n = vec2f(k.u.y, -k.u.x) * s;
    let ra = k.sa.h + k.sh;
    let rb = k.sb.h + k.sh;
    let ae = k.sa.p + n * (ra / k.mpu);
    let be = k.sb.p + n * (rb / k.mpu);
    let ab = be - ae;
    let lu = length(ab);
    let ue = select(k.u, ab / lu, lu > 0.0);
    let v = k.q - ae;
    e.along = dot(v, ue) * k.mpu;
    e.off = abs(m_cross(ue, v)) * k.mpu;
    e.len = lu * k.mpu;
    let foot = clamp(e.along, 0.0, e.len) - e.along;
    e.d = sqrt(foot * foot + e.off * e.off);
    // Past the edge into its paved side the cone falls on at face_max, as heights::groups.
    e.inside = max(-dot(v, n) - H_SIDE_UNITS, 0.0) * k.mpu * H.face_max;
    e.z = vec2f(k.sa.z + k.sa.bank * s * ra, k.sb.z + k.sb.bank * s * rb);
    let w = select(13u, 12u, s > 0.0);
    e.t = vec2f(h_known_word(k.wa + w), h_known_word(k.wa + M_SAMPLE + w));
    e.known = e.t > vec2f(H_UNKNOWN * 0.5);
    return e;
}

fn s_wall(k: Hit) -> f32 {
    return select(0.0, H.wall_max, (k.ln.rank & H_WALL) != 0u);
}

struct ConeOut {
    @location(0) h: f32,
    @builtin(frag_depth) depth: f32,
}

fn s_depth(h: f32) -> f32 {
    return clamp(0.5 + (h - sj.base) / S_SPAN, 0.0, 1.0);
}

// The line's fill at the texel over both edges of the segment, as heights::groups and h_close.
@fragment
fn fill_fragment(in: VOut) -> ConeOut {
    let k = s_hit(in.pos, in.inst);
    if (k.cut || k.d <= k.half + k.sh) {
        discard;
    }
    let wall = s_wall(k);
    var fill = -H_BIG;
    var near = H_BIG;
    for (var j = 0u; j < 2u; j++) {
        let e = s_edge(k, select(-1.0, 1.0, j == 0u));
        if (e.d >= k.ln.reach) {
            continue;
        }
        near = min(near, e.d);
        let f = h_fit(k.ln.fill, k.ln.fill_max, e.z - e.t, e.known, wall, k.ln.reach);
        fill = max(fill, h_edge_cone(e.along, e.off, e.len, e.z - f.yz, f.x, 1.0) - h_release(e.d, k.ln.reach).y - e.inside);
    }
    if (near >= H_BIG * 0.5) {
        discard;
    }
    fill += h_round_back(k.ln.fill, near);
    return ConeOut(fill, s_depth(fill));
}

// The line's cut at the texel, risen toward the fills there up to its steepest cut from its highest wall,
// as heights::slope's `hi` without the soft corners.
@fragment
fn cut_fragment(in: VOut) -> ConeOut {
    let k = s_hit(in.pos, in.inst);
    if (k.cut || k.d <= k.half + k.sh) {
        discard;
    }
    let wall = s_wall(k);
    let steepest = min(max(k.ln.cut_max, k.ln.cut), H.face_max);
    var cut = H_BIG;
    var steep = H_BIG;
    var near = H_BIG;
    for (var j = 0u; j < 2u; j++) {
        let e = s_edge(k, select(-1.0, 1.0, j == 0u));
        if (e.d >= k.ln.reach) {
            continue;
        }
        near = min(near, e.d);
        let go = h_release(e.d, k.ln.reach).y + e.inside;
        let c = h_fit(k.ln.cut, k.ln.cut_max, e.t - e.z, e.known, wall, k.ln.reach);
        cut = min(cut, h_edge_cone(e.along, e.off, e.len, e.z + c.yz, c.x, -1.0) + go);
        let top = e.z + wall;
        let s = h_fit(steepest, steepest, e.t - top, e.known, 0.0, k.ln.reach);
        steep = min(steep, h_edge_cone(e.along, e.off, e.len, top + s.yz, s.x, -1.0) + go);
    }
    if (near >= H_BIG * 0.5) {
        discard;
    }
    cut -= h_round_back(k.ln.cut, near);
    steep -= h_round_back(steepest, near);
    let fill = textureLoad(lower, vec2i(in.pos.xy), 0).x;
    let up = max(cut, min(fill, steep));
    return ConeOut(up, s_depth(up));
}
