// The ground plan's mesh of a cell (pieces.ts), drawn before the terrain with the corridors' fragment stage
// (PIECE): its triangles at the corners' heights from corners.wgsl, and its step slots, two triangles each
// on the slot's four corners. A terrain vertex of the strip is the terrain's own
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

// As NO_LABEL of corners.wgsl: the plain terrain.
const NO_LABEL: u32 = 33554431u;
// As `cracks` in DEBUG_VIEWS of view.ts.
const CRACKS_VIEW: u32 = 3u;
// A step beside a paved face this high is a stone wall.
const WALL_STEP_M: f32 = 0.5;
// A bank this high or lower shades as the ground beside it, this high or higher as a face of its own.
const BANK_LOW_M: f32 = 0.05;
const BANK_HIGH_M: f32 = 0.5;

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
        // At the finest level 17 the mesh lies over level-17 nodes: the vertex is the terrain's level-17 vertex at
        // the even point that it slides to, by the same code, so that it lands where the terrain's does (the strip
        // at morph 1). The mesh's range lies within level 18's, so at the finest level 18 its nodes are level 18.
        if (surface.finest < 18u) {
            let even = u - ((surface.anchor + u) & vec2i(64));
            let v = grid_vertex_by(even, 17u, !plan_near(even), true);
            return PieceVertex(vec2f(u), v.s, v.h);
        }
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

// A corner's look word marks a wall class's road surface (LOOK_WALL_SIDE of plan/corners.wgsl) in the bit of the
// corridors' fine offset, which pieces do not read.
const WALL_SIDE: u32 = 1u << FINE_SHIFT;

// Whether corner k lies on the road surface of a wall class.
fn on_road(k: u32) -> bool {
    return (piece[params.out + 4u * k + 1u] & WALL_SIDE) != 0u;
}

// Whether corner k lies on water (MAT_WATER of corners.wgsl).
fn on_water(k: u32) -> bool {
    return (piece[params.out + 4u * k + 1u] & MAT_BITS) == MAT_WATER;
}

// A corner's metres across and half-width, and its metres along, from its look and place words.
struct CornerPlace {
    road: vec2f,
    along: f32,
}

fn place_at(k: u32) -> CornerPlace {
    let o = params.out + 4u * k;
    return CornerPlace(vec2f(across_of(piece[o + 2u]), half_of(piece[o + 1u])), f32(piece[o + 2u] >> 12u) * ALONG_STEP);
}

// A corner's place in a ground corner's terms (corridor_ground): a paved corner lies on its way.
fn ground_place(k: u32) -> CornerPlace {
    let p = place_at(k);
    if ((piece[params.out + 4u * k + 1u] & MAT_BITS) == MAT_GROUND) {
        return p;
    }
    return CornerPlace(vec2f(abs(p.road.x) - p.road.y, p.road.y), p.along);
}

fn piece_out(v: PieceVertex, word: u32, place: CornerPlace, normal: vec3f) -> CorridorOut {
    var out: CorridorOut;
    let p = corner_point(v);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    // The debug views show the terrain alone, as the corridors do; the crack and owned views show the meshes too.
    if (frame.debug != 0u && frame.debug != CRACKS_VIEW && frame.debug != OWNED_VIEW) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = v.u;
    out.relative = p;
    out.altitude = v.h;
    out.word = word;
    out.road = place.road;
    out.along = place.along;
    out.normal = normal;
    return out;
}

@vertex
fn piece_vertex(@builtin(vertex_index) k: u32) -> CorridorOut {
    let o = params.out + 4u * k;
    var out = piece_out(corner_at(k), piece[o + 1u], place_at(k), octahedral(unpack2x16snorm(piece[o + 3u])));
    if (MOUTH) {
        // The corner's label, as corners.wgsl reads it.
        let label = (piece[2u * k] >> 19u) | (((piece[2u * k + 1u] >> 19u) & 0xfffu) << 13u);
        corridor_label(&out, select(label, 0xffffffffu, label == NO_LABEL));
    }
    return out;
}

// Six vertices per step slot a_p, a_q, b_q, b_p: a lies right of p → q from above, b left. The slot faces its
// lower side. Its two triangles take only its four corners, so every edge it shares with a face or a
// neighbour slot is that edge exactly and no pixel falls between them at any heights. Where the step turns,
// the diagonal joins the lower corners at p and q: the fold then hangs below both faces, hidden.
@vertex
fn slot_vertex(@builtin(vertex_index) index: u32) -> CorridorOut {
    let w = params.slots + 4u * (index / 6u);
    var c = array(corner_at(piece[w]), corner_at(piece[w + 1u]), corner_at(piece[w + 2u]), corner_at(piece[w + 3u]));
    let dp = c[0].h - c[3].h;
    let dq = c[1].h - c[2].h;
    let turns = dp * dq < 0.0;
    // The diagonal a_p, b_q, or a_q, b_p where a stands higher at p and lower at q.
    var order = array(0u, 1u, 2u, 0u, 2u, 3u);
    if (turns && dp > 0.0) {
        order = array(0u, 1u, 3u, 1u, 2u, 3u);
    }
    let at = order[index % 6u];
    let v = c[at];
    var place = ground_place(piece[w + at]);
    let end = select(dq, dp, at == 0u || at == 3u);
    // The rise of a over b: of the slot, or where it turns, of the triangle's visible lobe: the first
    // triangle holds the lobe where a stands higher, the second the one where b does.
    let rise = select(dp + dq, select(min(dp, dq), max(dp, dq), index % 6u < 3u), turns);
    let step = abs(end);
    // Toward b, the left of p → q, where a stands higher.
    let along = c[1].s - c[0].s;
    let left = normalize(vec2f(-along.y, along.x) + vec2f(1e-9, 0.0));
    // A step whose upper side is water falls as water (its look foams on the upright slot). A step beside a wall
    // class's road surface that stands as high as a wall is one; any other is a bank of the ground, as on a
    // path or a track.
    let upper = select(vec2u(piece[w + 2u], piece[w + 3u]), vec2u(piece[w], piece[w + 1u]), rise > 0.0);
    var mat = select(MAT_GROUND, MAT_WALL, (on_road(piece[w]) || on_road(piece[w + 3u])) && max(abs(dp), abs(dq)) >= WALL_STEP_M);
    if (on_water(upper.x) || on_water(upper.y)) {
        mat = MAT_WATER;
    }
    // A bank's level normal is as long as its share of the light: a step of millimetres shades as the ground
    // beside it, a real one as a face of its own (corridor_ground).
    let share = select(smoothstep(BANK_LOW_M, BANK_HIGH_M, step), 1.0, mat == MAT_WALL || mat == MAT_WATER);
    let normal = vec3f(select(-left, left, rise > 0.0) * share, 0.0);
    // A wall whose road stands on its higher side holds the road up, and the face that it continues lies below it; else
    // that face lies above it (wall_rock of roads/corridor.wgsl).
    let fill = select(on_road(piece[w + 3u]), on_road(piece[w]), rise > 0.0);
    let word = mat | (480u << HALF_SHIFT) | select(0u, WALL_FILL, mat == MAT_WALL && fill);
    if (mat == MAT_WALL) {
        place = CornerPlace(vec2f(v.h - min(c[0].h, c[3].h), half_of(word)), length(v.s - c[0].s));
    }
    return piece_out(v, word, place, normal);
}
