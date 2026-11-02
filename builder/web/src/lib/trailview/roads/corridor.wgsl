diagnostic(off, derivative_uniformity);
// The corridors of roads/: swept meshes of each road, track and path within the range, drawn before the
// terrain. A band's rows stand on the stamped ground, read in the vertex stage; every other vertex stands
// at its height from the worker. A face takes its material from its first vertex. A look's share of the ground mixes
// the ground's colour into the way, so an edge stays in place as the eye moves and fades as the paint's does. What is
// all ground, and the share that the paint takes near the end of the corridors' reach, discard: the terrain draws
// those pixels once. The paint's share draws by a dither, and the frames resolve it. The parts of parts.ts are
// instances that share the fragment stage.
// #include ../ground/look.wgsl
// #include looks.wgsl

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

// As piece.ts.
const CORRIDOR_STEP: f32 = 0.083333336;
const HALF_STEP_M: f32 = 0.025;
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
const CF_SAMPLE: u32 = 1u;
const CF_MESH: u32 = 2u;
const CF_FILL: u32 = 3u;
const CF_UNDER: u32 = 4u;

// Where the ground climbs or falls BAND_STEEP per metre from the core's outer edge out to a band's row, the
// drawn mesh cannot follow the ground that the band reads, and the two would fight: the band dives
// BAND_DIVE_M under the mesh, which shows alone. A cut's and a fill's cones start at 0.85 (roads.json).
const BAND_DIVE_M: f32 = 1.5;
const BAND_STEEP_LOW: f32 = 0.6;
const BAND_STEEP_HIGH: f32 = 0.8;
// The nearest a row counts from the core's edge, in metres.
const BAND_LEAST_M: f32 = 0.3;
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

struct CorridorOut {
    @builtin(position) clip: vec4f,
    // Units about the anchor.
    @location(0) u: vec2f,
    @location(1) relative: vec3f,
    @location(2) altitude: f32,
    @location(3) @interpolate(flat) word: u32,
    // Metres across and the half-width.
    @location(4) road: vec2f,
    @location(5) along: f32,
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

// The stamped ground: one direct page lookup at the level that the terrain draws there, one level coarser
// while that page is on its way, else the road's plane.
fn band_ground(s: vec2f, plane: f32) -> f32 {
    let level = u32(ceil(lod(s)));
    let fine = bilinear(level, texel_of(s, level));
    if (known(fine)) {
        return fine;
    }
    let coarse = bilinear(level - 1u, texel_of(s, level - 1u));
    return select(plane, coarse, known(coarse));
}

// How far a row `out` metres past the core's edge dives under `ground`, on a road of plane height `plane`.
fn band_dive(ground: f32, plane: f32, out: f32) -> f32 {
    return BAND_DIVE_M * smoothstep(BAND_STEEP_LOW, BAND_STEEP_HIGH, abs(ground - plane) / max(out, BAND_LEAST_M));
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
    return out;
}

@vertex
fn part_caster(v: PartIn) -> @builtin(position) vec4f {
    return frame.view_proj * vec4f(part_at(v, part_point(v)).xyz, 1.0);
}

// The ground at a pixel of a corridor; a path's band reads no mask.
fn corridor_ground(in: CorridorOut, su: vec2f, d: mat2x3f) -> vec3f {
    return ground_colour(su, in.relative, d, in.altitude, u32(ceil(lod(su))), in.clip.xy, select(0.0, 1.0, in.road.y < PATH_HALF_M));
}

@fragment
fn corridor_fragment(in: CorridorOut) -> @location(0) vec4f {
    if (paint_draws(length(in.relative.xy), in.clip.xy)) {
        discard;
    }
    let mat = in.word & MAT_BITS;
    if (mat >= LOOK_TREAD && mat <= LOOK_CABLE && dither(in.clip.xy + DETAIL_DITHER) < detail_fade(length(in.relative.xy))) {
        discard;
    }
    let su = lod_space(in.u);
    let d = mat2x3f(dpdx(in.relative), dpdy(in.relative));
    if (mat == MAT_GROUND) {
        return vec4f(corridor_ground(in, su, d), 1.0);
    }
    var n = normalize(cross(d[0], d[1]));
    n *= -sign(dot(n, in.relative));
    let p = look.place.xy + in.relative.xy;
    let pw = max(length(d[0]), length(d[1])) * look.screen.z;
    let cw = max(length(vec2f(dpdx(in.road.x), dpdy(in.road.x))), 1e-4);
    let aw = max(length(vec2f(dpdx(in.along), dpdy(in.along))), 1e-4);
    let way = WayPixel(p, in.road.x, in.road.y, in.along, pw, cw, aw);
    let code = (in.word >> CODE_SHIFT) & 15u;
    let l = way_look(mat, code, region_at(su), su, way, (in.word >> FLAGS_SHIFT) & FLAGS_BITS, n.z, in.altitude);
    if (l.ground >= 1.0) {
        discard;
    }
    if (l.ground > 0.0) {
        return vec4f(mix(margin_lit(l, n, su, in.relative, in.altitude), corridor_ground(in, su, d), l.ground), 1.0);
    }
    if (mat == LOOK_RISER && soil_steps(code)) {
        return vec4f(riser_lit(l, n, su, in.relative, in.altitude), 1.0);
    }
    return vec4f(way_lit(l, n, su, in.relative, in.altitude), 1.0);
}
