diagnostic(off, derivative_uniformity);
// The corridors of roads/: swept meshes of each road, track and path within the range, drawn before the
// terrain. A band's rows stand on the stamped ground, read in the vertex stage; every other vertex stands
// at its height from the worker. A face takes its material from its first vertex. A look's share of the ground mixes
// the ground's colour into the way, so an edge stays in place as the eye moves and fades as the paint's does. What is
// all ground, and the share that the paint takes near the end of the corridors' reach, discard: the terrain draws
// those pixels once. The paint's share draws by a dither, and the frames resolve it. The parts of parts.ts are
// instances that share the fragment stage. So do the pieces of the ground plan's mesh (plan/piece.wgsl), with PIECE.
// #include ../ground/look.wgsl
// #include ../terrain/drawn-mesh.wgsl
// #include band.wgsl
// #include looks.wgsl
// #include corridor-in.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    pixel: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
// The debug view `owned` of DEBUG_VIEWS (view.ts).
const OWNED_VIEW: u32 = 11u;

// As piece.ts.
const CORRIDOR_STEP: f32 = 0.083333336;
const ACROSS_STEP_M: f32 = 0.0078163;
const ALONG_STEP: f32 = 0.015625;
const MAT_BITS: u32 = 127u;
const CODE_SHIFT: u32 = 7u;
const FLAGS_SHIFT: u32 = 11u;
const FLAGS_BITS: u32 = 31u;
const PRIO_SHIFT: u32 = 16u;
const HALF_SHIFT: u32 = 19u;
const HALF_BITS: u32 = 511u;
const FINE_SHIFT: u32 = 28u;
// As MAT, LOOK and CF of section.ts.
const MAT_GROUND: u32 = 1u;
// In a piece's wall's word, the code bit (1 << CODE_SHIFT) that its look does not read: its road stands on its higher
// side (plan/piece.wgsl).
const WALL_FILL: u32 = 128u;
const CF_SAMPLE: u32 = 1u;
const CF_MESH: u32 = 2u;
const CF_FILL: u32 = 3u;
const CF_UNDER: u32 = 4u;

// The outer row lies under the stamped ground by as much as a cell of the finest mesh leaves it at a crease.
const BAND_SINK_M: f32 = 0.3;
// A row over another way's carriageway sinks this far, so that way draws over it.
const BAND_UNDER_M: f32 = 0.2;
// A better band stands this far per step of its priority over another where two overlap.
const BAND_PRIO_M: f32 = 0.004;
// A corridor draws this far toward the eye, plus a share of its distance: where its band and the terrain
// hold the same ground the band wins, and a hill in front of a road still hides it.
const CORRIDOR_BIAS_M: f32 = 0.06;
const CORRIDOR_BIAS_SHARE: f32 = 0.003;
// A wall's face and capstone draw this far nearer still: a coarse triangle of the terrain can cut the crease at a
// wall's foot and stand in front of it.
const WALL_BIAS_M: f32 = 0.3;
// A piece of the ground plan's mesh: no paint hands over to it and no terrain stands under it, so it draws
// every pixel, its ground too, lit by its own normal.
override PIECE: bool = false;
// A stop arm's junction faces of the ground plan's mesh (plan/pieces.ts): their pixels find the mouth (corridor_in).
override MOUTH: bool = false;
// A piece that reaches past the level-18 plateau of the LOD fades out by a dither across the band where the
// terrain morphs to level 17, and the terrain's far look shows through: the frames resolve the two.
override CUT: bool = false;
// A piece's edge looks (lines, fray, grass) lie within this of its frame's edge. Its edge opens by how much farther
// the paved area goes on past the frame's edge, from the first to the second.
const EDGE_LOOK_M: f32 = 0.6;
const OPEN_FROM_M: f32 = 0.15;
const OPEN_TO_M: f32 = 0.35;
// Moves the dither of a detail's fade off the paint's.
const DETAIL_DITHER: vec2f = vec2f(61.0, 7.0);
// As piece.ts: an instance's fine place, turn and grade.
const PART_FINE: f32 = 16.0;
const PART_TURN: f32 = 65536.0;
const PART_GRADE: f32 = 64.0;

struct CorridorIn {
    // The high bits of the steps of CORRIDOR_STEP units about `middle`; the word holds their low bits.
    @location(0) at: vec2i,
    // The height; on a band's row, the road's plane there.
    @location(1) z: f32,
    // The fields of `lookWord` of piece.ts.
    @location(2) word: u32,
    // Metres across in the low 12 bits, signed; the count along above them.
    @location(3) place: u32,
    // The middle of the piece, in whole world units.
    @location(4) middle: vec2i,
}

struct PartIn {
    // Metres across to the left, ahead and up from the instance's point.
    @location(0) at: vec3f,
    @location(1) word: u32,
    // The point in whole world units.
    @location(2) point: vec2i,
    @location(3) z: f32,
    // Its turn from its low bit (16 bits), its fine x and y (4 each), and its grade (8, signed).
    @location(4) pose: u32,
}

// The height of a band's row. `edge` is the metres of the core's outer edge from the middle.
fn band_height(s: vec2f, plane: f32, word: u32, flags: u32, across: f32, edge: f32) -> f32 {
    let ground = band_ground(s, plane);
    if ((flags & CF_MESH) != 0u) {
        // A backfill's row holds its height where the ground lies over it.
        return select(ground - BAND_SINK_M, min(plane, ground - BAND_SINK_M), (flags & CF_FILL) == CF_FILL);
    }
    let prio = f32((word >> PRIO_SHIFT) & 7u);
    return ground + prio * BAND_PRIO_M - band_dive(ground, plane, abs(across) - edge) - select(0.0, BAND_UNDER_M, (flags & CF_UNDER) != 0u);
}

// A point at render-space `p` about the eye, moved toward the eye.
fn corridor_biased(p: vec3f, wall: bool) -> vec3f {
    let d = length(p);
    return p * max(0.0, 1.0 - (CORRIDOR_BIAS_M + select(0.0, WALL_BIAS_M, wall) + CORRIDOR_BIAS_SHARE * d) / max(d, 1e-3));
}

fn across_of(place: u32) -> f32 {
    return f32(bitcast<i32>(place << 20u) >> 20u) * ACROSS_STEP_M;
}

fn half_of(word: u32) -> f32 {
    return f32((word >> HALF_SHIFT) & HALF_BITS) * HALF_STEP_M;
}

// Units about the anchor of a vertex.
fn corridor_at(v: CorridorIn) -> vec2f {
    let fine = vec2i(vec2u(v.word >> FINE_SHIFT, v.word >> (FINE_SHIFT + 2u)) & vec2u(3u));
    return vec2f(v.middle - surface.anchor) + vec2f(v.at * 4 + fine) * CORRIDOR_STEP;
}

@vertex
fn corridor_vertex(v: CorridorIn) -> CorridorOut {
    let u = corridor_at(v);
    let s = lod_space(u);
    let flags = (v.word >> FLAGS_SHIFT) & FLAGS_BITS;
    let across = across_of(v.place);
    let half = half_of(v.word);
    let z = select(v.z, band_height(s, v.z, v.word, flags, across, half), (flags & CF_FILL) != 0u);
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: CorridorOut;
    let mat = v.word & MAT_BITS;
    out.clip = frame.view_proj * vec4f(corridor_biased(p, mat == MAT_WALL || mat == MAT_CAP), 1.0);
    // The debug views show the terrain alone: past the near plane, the triangle clips.
    if (frame.debug != 0u) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = u;
    out.relative = p;
    out.altitude = z;
    out.word = v.word;
    out.road = vec2f(across, half);
    out.along = f32(v.place >> 12u) * ALONG_STEP;
    out.normal = vec3f(0.0);
    return out;
}

// A vertex of a caster, which stands at its height: the bias would shadow the band.
@vertex
fn corridor_caster(v: CorridorIn) -> @builtin(position) vec4f {
    let s = lod_space(corridor_at(v));
    let flags = (v.word >> FLAGS_SHIFT) & FLAGS_BITS;
    let z = select(v.z, band_height(s, v.z, v.word, flags, 0.0, 0.0), (flags & CF_FILL) != 0u);
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    return frame.view_proj * vec4f(p, 1.0);
}

// Units about the anchor of a part's point.
fn part_point(v: PartIn) -> vec2f {
    return vec2f(v.point - surface.anchor) + vec2f(f32((v.pose >> 16u) & 15u), f32((v.pose >> 20u) & 15u)) / PART_FINE;
}

// A vertex of a part: xyz in render space about the eye, w its height.
fn part_at(v: PartIn, u: vec2f) -> vec4f {
    let turn = f32(v.pose & 65535u) / PART_TURN * 6.2831853;
    let left = vec2f(cos(turn), sin(turn));
    let z = v.z + v.at.z + f32(bitcast<i32>(v.pose) >> 24u) / PART_GRADE * v.at.y;
    var p = vec3f(to_render(lod_space(u) + left * v.at.x + vec2f(left.y, -left.x) * v.at.y), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    return vec4f(p, z);
}

@vertex
fn part_vertex(v: PartIn) -> CorridorOut {
    let u = part_point(v);
    let p = part_at(v, u);
    var out: CorridorOut;
    out.clip = frame.view_proj * vec4f(corridor_biased(p.xyz, false), 1.0);
    if (frame.debug != 0u) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = u;
    out.relative = p.xyz;
    out.altitude = p.w;
    out.word = v.word;
    out.road = vec2f(v.at.x, 0.0);
    out.along = v.at.y;
    out.normal = vec3f(0.0);
    return out;
}

@vertex
fn part_caster(v: PartIn) -> @builtin(position) vec4f {
    return frame.view_proj * vec4f(part_at(v, part_point(v)).xyz, 1.0);
}

// The ground at a pixel of a corridor; a path's band reads no mask. A piece's ground is the terrain's: the
// pages' normal and the texels of how far the heights moved the ground, which the stamp made from the same
// heights, so it shades as the terrain beside it does. A slope's daylight line runs through the mesh's
// triangles, whose vertices would draw it as a sawtooth. Its nearest way is its corners': a ground corner
// holds the metres past that way's edge and its half-width (plan/corners.wgsl), a paved face's pixel its own
// way's. The mask's texels give no half-width far from a way's middle.
fn corridor_ground(in: CorridorOut, su: vec2f, d: mat2x3f) -> vec3f {
    if (PIECE) {
        // A step's bank: its level normal, as long as its share of the light (slot_vertex of plan/piece.wgsl).
        var n = drawn_relief(su);
        let share = length(in.normal);
        if (abs(in.normal.z) < 0.01 && share > 0.0) {
            n = vec4f(normalize(mix(n.xyz, in.normal / share, min(share, 1.0))), n.w);
        }
        return piece_ground(in, su, d, n, 0.0);
    }
    return ground_colour(su, in.relative, d, in.altitude, u32(ceil(lod(su))), in.clip.xy, band_road(select(0.0, 1.0, in.road.y < PATH_HALF_M)), drawn_relief(su));
}

// A piece's ground at a pixel, lit and looked by the normal `n` in LOD-space axes and its turn per texel in w
// (ground_colour); `wall` 1 for a wall that stands as rock (wall_rock), which ground_colour draws all rock.
fn piece_ground(in: CorridorOut, su: vec2f, d: mat2x3f, n: vec4f, wall: f32) -> vec3f {
    let level = lod(su);
    let edge = select(abs(in.road.x) - in.road.y, in.road.x, (in.word & MAT_BITS) == MAT_GROUND);
    let g = model_reach(level, length(in.relative.xy)) * ground_terms(moved_at(su, u32(ceil(level))), edge, in.road.y);
    return ground_colour(su, in.relative, d, in.altitude, u32(ceil(level)), in.clip.xy, Road(vec3f(0.0), 0.0, 0.0, max(g.x, wall), g.y, g.z), n);
}

// The face beside a piece's wall at a point of the wall, on the side `out` (a unit vector of LOD space from the wall
// into it): x its rise per metre from one to three texels from the wall, of the finest level up to `top` that knows both
// points, and y the metres that the heights moved it WALL_MOVED_M out, from the masks of `level`. A nearer texel would
// hold the wall's own step; over two texels the read passes the round at the face's foot (`round_m`, 0.5 m).
fn side_read(s: vec2f, out: vec2f, top: u32, level: u32) -> vec2f {
    for (var k = top; k >= COARSEST; k--) {
        let t = f32(1 << (22u - k)) * surface.unit;
        let a = bilinear_rise(k, texel_of(s + t * out, k)).x;
        let b = bilinear_rise(k, texel_of(s + 3.0 * t * out, k)).x;
        if (known(a) && known(b)) {
            return vec2f((b - a) / (2.0 * t), moved_at(s + WALL_MOVED_M * out, level));
        }
    }
    return vec2f(0.0);
}

// A wall reads how far the heights moved its face this far out, in metres so that levels 17 and 18 read the same ground.
const WALL_MOVED_M: f32 = 1.0;

// A wall reads its face at knots this far apart along the world axis nearer to the wall, and between two knots its rock
// share and its light run linearly: they change along a wall over metres, never at each bump of the ground or at each
// edge of a cut face, and the knots stay where they are as the eye moves. (Three reads a knot gave the same walls and
// cost 0.12 to 0.18 ms more on the portal flights.)
const WALL_KNOT_M: f32 = 8.0;

// What a wall needs of its face that rock_share does not read from the wall's own ground: the wall's ground `g`, the z
// of its coarse normal, the reach of the one road model there and the half-width of its word.
struct WallGround {
    g: Ground,
    coarse_z: f32,
    reach: f32,
    half: f32,
}

// The face beside a piece's wall at `s` (`world` its world metres), on the side `out`, `along` the wall's direction: x
// the rock share that rock_share gives it and y its rise per metre, from the knots about it on the world axis nearer to
// the wall (y, x, or both cross-faded within about 6° of 45°, as the masonry's).
fn wall_side(s: vec2f, world: vec2f, out: vec2f, along: vec2f, top: u32, level: u32, w: WallGround) -> vec2f {
    let north = smoothstep(-0.1, 0.1, (abs(along.y) - abs(along.x)) / (abs(along.x) + abs(along.y) + 1e-6));
    var face = vec2f(0.0);
    if (north < 1.0) {
        face = knot_face(s, world.x, along.x, out, along, top, level, w);
    }
    if (north > 0.0) {
        face = mix(face, knot_face(s, world.y, along.y, out, along, top, level, w), north);
    }
    return face;
}

// wall_side between the two knots about `u` (the world axis's metres at s), `du` the axis's share of `along`: the rock
// share of each knot from its own read, blended linearly, so the share changes over metres whatever thresholds lie in
// rock_share; and the rise, blended alike.
fn knot_face(s: vec2f, u: f32, du: f32, out: vec2f, along: vec2f, top: u32, level: u32, w: WallGround) -> vec2f {
    let u0 = floor(u / WALL_KNOT_M) * WALL_KNOT_M;
    let step = along / select(du, 1e-3, abs(du) < 1e-3);
    let a = side_read(s + (u0 - u) * step, out, top, level);
    let b = side_read(s + (u0 + WALL_KNOT_M - u) * step, out, top, level);
    let f = (u - u0) / WALL_KNOT_M;
    return vec2f(mix(knot_rock(a, out, w), knot_rock(b, out, w), f), mix(a.x, b.x, f));
}

// The rock share of a knot's read (side_read) as rock_share.
fn knot_rock(read: vec2f, out: vec2f, w: WallGround) -> f32 {
    let z = render_normal(normalize(vec3f(-read.x * out, 1.0))).z;
    return rock_share(w.g, z, w.coarse_z, w.reach * ground_terms(read.y, 0.0, w.half).y);
}

// A piece's wall continues the face beside it away from its road: below a wall that holds its road up, above a wall
// that its road cuts. It stands as rock where rock_share draws that face partly as rock, so a vertical face beside rock
// holds no soil and no wall is half of masonry. The share is taken at each knot from that knot's own read (the moved
// ground WALL_MOVED_M out) and blended between the knots, so it changes along a wall only over metres. A rock wall is
// lit by that face's normal. Under trees and between the legs of a hairpin a wall stays masonry. Only on steep land:
// elsewhere the wall reads one texel of the coarse relief.
struct WallRock {
    share: f32,
    face: vec3f,
}

fn wall_rock(in: CorridorOut, su: vec2f) -> WallRock {
    var out = WallRock(0.0, vec3f(0.0, 0.0, 1.0));
    let lambda = lod(su);
    let level = u32(ceil(lambda));
    let coarse_z = coarse_z_at(level, su);
    if (coarse_steep_of(coarse_z) <= 0.0) {
        return out;
    }
    let top = min(surface.finest, 18u);
    let lower = normalize(in.normal.xy + vec2f(1e-9, 0.0));
    let side = select(-lower, lower, (in.word & WALL_FILL) != 0u);
    let world = look.place.xy + in.relative.xy;
    let w = WallGround(ground_at(level, su, world, 1e9), coarse_z, model_reach(lambda, length(in.relative.xy)), in.road.y);
    let face = wall_side(su, world, side, vec2f(-lower.y, lower.x), top, level, w);
    out.face = normalize(vec3f(-face.y * side, 1.0));
    out.share = smoothstep(0.1, 0.4, face.x);
    return out;
}

@fragment
fn corridor_fragment(raw: CorridorOut) -> @location(0) vec4f {
    var in = raw;
    if (MOUTH) {
        in = corridor_in(raw);
    }
    if (!PIECE && paint_draws(length(in.relative.xy), in.clip.xy)) {
        discard;
    }
    // Whole two level-17 cells past where the terrain sinks (level 18's morph start), as far as its sunk
    // corners' triangles reach and the terrain takes no snap (PLAN_TRIANGLE_UNITS of cells.wgsl); gone
    // where level 18 ends. In metres, so the same at the finest level 17.
    if (PIECE && CUT) {
        let sink = surface.range * exp2(-18.0 - MORPH);
        let d = length(vec3f(lod_space(in.u), surface.above));
        if (dither(in.clip.xy + DETAIL_DITHER) >= 1.0 - smoothstep(sink + 256.0 * surface.unit, surface.range * exp2(-18.0), d)) {
            discard;
        }
    }
    let mat = in.word & MAT_BITS;
    if (!PIECE && mat >= LOOK_TREAD && mat <= LOOK_LOG && dither(in.clip.xy + DETAIL_DITHER) < detail_fade(length(in.relative.xy))) {
        discard;
    }
    let su = lod_space(in.u);
    let d = mat2x3f(dpdx(in.relative), dpdy(in.relative));
    // The owned view: a piece in green.
    if (PIECE && frame.debug == OWNED_VIEW) {
        return vec4f(0.0, 0.5, 0.0, 1.0);
    }
    // The crack view: a piece in grey, as the terrain.
    if (PIECE && frame.debug == 3u) {
        return vec4f(vec3f(0.25 + 0.5 * (max(dot(render_normal(select(normalize(in.normal), drawn_normal(su), (in.word & MAT_BITS) == MAT_GROUND)), frame.sun.xyz), 0.0) * frame.sun.w + 0.3) / 1.3), 1.0);
    }
    if (mat == MAT_GROUND) {
        return vec4f(corridor_ground(in, su, d), 1.0);
    }
    // A piece's wall in a cliff is a rock cut (wall_rock).
    var rock = WallRock(0.0, vec3f(0.0, 0.0, 1.0));
    if (PIECE && mat == MAT_WALL) {
        rock = wall_rock(in, su);
        if (rock.share >= 1.0) {
            return vec4f(piece_ground(in, su, d, vec4f(rock.face, 0.0), 1.0), 1.0);
        }
    }
    var n = normalize(cross(d[0], d[1]));
    n *= -sign(dot(n, in.relative));
    if (PIECE) {
        n = render_normal(normalize(in.normal));
    }
    let p = look.place.xy + in.relative.xy;
    let pw = max(length(d[0]), length(d[1])) * look.screen.z;
    let cw = max(length(vec2f(dpdx(in.road.x), dpdy(in.road.x))), 1e-4);
    let aw = max(length(vec2f(dpdx(in.along), dpdy(in.along))), 1e-4);
    // A piece's carriageway keeps its pixels within the half-width (PIECE_INSIDE of plan/corners.wgsl). A pixel past
    // it lies where the face goes on past its frame's edge, as in a junction's mouth: that edge opens there.
    let past = PIECE && ((in.word >> PRIO_SHIFT) & 1u) == 1u && abs(in.road.x) > in.road.y;
    let across = select(in.road.x, clamp(in.road.x, -in.road.y, in.road.y), past);
    // A face edge between two paved faces, as a way's toward a junction's mouth, lies inside the paved area: a piece
    // draws its edge only where the frame's edge is the paved outline's, which the far look draws too (the paint).
    var open = 0.0;
    let inside = in.road.y - abs(in.road.x);
    if (PIECE && inside < EDGE_LOOK_M) {
        open = smoothstep(OPEN_FROM_M, OPEN_TO_M, -paved_edge(su, u32(ceil(lod(su)))) - inside);
    }
    let way = WayPixel(p, across, in.road.y, in.along, pw, cw, aw, open);
    let code = (in.word >> CODE_SHIFT) & 15u;
    let flags = ((in.word >> FLAGS_SHIFT) & FLAGS_BITS) | select(0u, CF_NO_LEFT_LINE | CF_NO_RIGHT_LINE, past);
    let l = way_look(mat, code, region_at(su), su, way, flags, n.z, in.altitude);
    if (PIECE && l.ground >= 1.0) {
        return vec4f(corridor_ground(in, su, d), 1.0);
    }
    if (l.ground >= 1.0) {
        discard;
    }
    if (l.ground > 0.0) {
        return vec4f(mix(margin_lit(l, n, su, in.relative, in.altitude), corridor_ground(in, su, d), l.ground), 1.0);
    }
    if (mat == LOOK_LOG) {
        return vec4f(log_lit(l, n, su, in.relative, in.altitude), 1.0);
    }
    if (rock.share > 0.0) {
        return vec4f(mix(way_lit(l, n, su, in.relative, in.altitude), piece_ground(in, su, d, vec4f(rock.face, 0.0), 1.0), rock.share), 1.0);
    }
    return vec4f(way_lit(l, n, su, in.relative, in.altitude), 1.0);
}
