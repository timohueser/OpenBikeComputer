// The ground plan's mesh of a cell (pieces.ts), drawn before the terrain with the corridors' fragment stage
// (PIECE): its triangles at the corners' heights from corners.wgsl, and its step slots, two triangles each
// that meet at the zero point where the step turns. A terrain vertex of the strip is the terrain's own
// vertex at its level-18 grid point, made by the same code, so the strip meets the terrain without a crack.
// #include ../roads/corridor.wgsl
// #include cells.wgsl

// As Piece of corners.wgsl.
struct Piece {
    origin: vec2i,
    corners: u32,
    slots: u32,
    out: u32,
    steps: u32,
    block: u32,
    first: u32,
}

@group(0) @binding(1) var<storage, read> piece: array<u32>;
@group(0) @binding(2) var<uniform> params: Piece;

// A step beside a paved face this high is a stone wall.
const WALL_STEP_M: f32 = 0.5;

struct PieceVertex {
    u: vec2f,
    s: vec2f,
    h: f32,
}

// A corner's place and height: a terrain vertex as the terrain's vertex_main makes it, any other at its stored height.
fn corner_at(k: u32) -> PieceVertex {
    let w0 = piece[2u * k];
    let w1 = piece[2u * k + 1u];
    let q = vec2u(w0 & 0x7ffffu, w1 & 0x7ffffu);
    if ((w1 >> 31u) == 1u) {
        let u = params.origin - surface.anchor + vec2i(q / 16u);
        let v = grid_vertex_by(u, 18u, !plan_near(u), true);
        return PieceVertex(vec2f(u), v.s, v.h);
    }
    let u = vec2f(params.origin - surface.anchor) + vec2f(q) / 16.0;
    return PieceVertex(u, lod_space(u), bitcast<f32>(piece[params.out + 4u * k]));
}

// As vertex_main of terrain.wgsl, so that a terrain vertex lands bit for bit where the terrain's does: render space about the eye.
fn corner_point(v: PieceVertex) -> vec3f {
    var p = vec3f(to_render(v.s), select(-surface.above, v.h - frame.eye, known(v.h)));
    p.z -= frame.curvature * dot(p.xy, p.xy);
    return p;
}

fn octahedral(e: vec2f) -> vec3f {
    var n = vec3f(e, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        n = vec3f((1.0 - abs(n.yx)) * select(vec2f(-1.0), vec2f(1.0), n.xy >= vec2f(0.0)), n.z);
    }
    return normalize(n);
}

fn piece_out(v: PieceVertex, word: u32, place: u32, normal: vec3f) -> CorridorOut {
    var out: CorridorOut;
    let p = corner_point(v);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    // The debug views show the terrain alone, as the corridors do.
    if (frame.debug != 0u) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = v.u;
    out.relative = p;
    out.altitude = v.h;
    out.word = word;
    out.road = vec2f(across_of(place), half_of(word));
    out.along = f32(place >> 12u) * ALONG_STEP;
    out.normal = normal;
    return out;
}

@vertex
fn piece_vertex(@builtin(vertex_index) k: u32) -> CorridorOut {
    let o = params.out + 4u * k;
    return piece_out(corner_at(k), piece[o + 1u], piece[o + 2u], octahedral(unpack2x16snorm(piece[o + 3u])));
}

// Six vertices per step slot a_p, a_q, b_q, b_p: a lies right of p → q from above, b left. The slot faces its
// lower side, with a level normal; where it turns, its two triangles meet at the zero point.
@vertex
fn slot_vertex(@builtin(vertex_index) index: u32) -> CorridorOut {
    let w = params.slots + 4u * (index / 6u);
    var c = array(corner_at(piece[w]), corner_at(piece[w + 1u]), corner_at(piece[w + 2u]), corner_at(piece[w + 3u]));
    let dp = c[0].h - c[3].h;
    let dq = c[1].h - c[2].h;
    let turns = dp * dq < 0.0;
    let t = select(0.0, dp / (dp - dq), turns);
    let corner = index % 6u;
    var order = array(0u, 1u, 2u, 0u, 2u, 3u);
    var v = c[order[corner]];
    // The rise of a over b at the triangle.
    var rise = dp + dq;
    if (turns) {
        // (a_p, zero, b_p), (zero, a_q, b_q).
        let zero = PieceVertex(mix(c[0].u, c[1].u, t), mix(c[0].s, c[1].s, t), mix(c[0].h, c[1].h, t));
        var split = array(0u, 0u, 3u, 0u, 1u, 2u);
        v = c[split[corner]];
        if (corner == 1u || corner == 3u) {
            v = zero;
        }
        rise = select(dq, dp, corner < 3u);
    }
    // Toward b, the left of p → q, where a stands higher.
    let along = c[1].s - c[0].s;
    let left = normalize(vec2f(-along.y, along.x) + vec2f(1e-9, 0.0));
    let normal = vec3f(select(-left, left, rise > 0.0), 0.0);
    // A step beside a paved face that stands as high as a wall is one; any other is a bank of the ground.
    let paved = (piece[params.out + 4u * piece[w] + 1u] & MAT_BITS) != MAT_GROUND || (piece[params.out + 4u * piece[w + 3u] + 1u] & MAT_BITS) != MAT_GROUND;
    let mat = select(MAT_GROUND, MAT_WALL, paved && max(abs(dp), abs(dq)) >= WALL_STEP_M);
    return piece_out(v, look_word_of(mat), place_of(v.h - min(c[0].h, c[3].h), length(v.s - c[0].s)), normal);
}

fn look_word_of(mat: u32) -> u32 {
    return mat | (480u << HALF_SHIFT);
}

fn place_of(across: f32, along: f32) -> u32 {
    return (bitcast<u32>(i32(round(clamp(across, -16.0, 16.0) / ACROSS_STEP_M))) & 0xfffu) | (u32(clamp(round(along / ALONG_STEP), 0.0, 1048575.0)) << 12u);
}
