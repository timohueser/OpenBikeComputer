diagnostic(off, derivative_uniformity);
// The corridors of roads/: swept meshes of each road, track and path within the range, drawn before the
// terrain. A band's rows stand on the stamped ground, read in the vertex stage; every other vertex stands
// at its height from the bake. A face takes its material from its first vertex.
// #include ../ground/look.wgsl

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
const CORRIDOR_STEP: f32 = 0.33333334;
const HALF_STEP_M: f32 = 0.025;
const ACROSS_STEP_M: f32 = 0.0078163;
const ALONG_STEP: f32 = 0.015625;
const CODE_SHIFT: u32 = 7u;
const FLAGS_SHIFT: u32 = 11u;
const PRIO_SHIFT: u32 = 16u;
const HALF_SHIFT: u32 = 19u;
// As MAT, LOOK and CF of section.ts.
const MAT_GROUND: u32 = 1u;
const MAT_KERB: u32 = 2u;
const MAT_SIDEWALK: u32 = 3u;
const LOOK_FIRST: u32 = 20u;
const CF_SAMPLE: u32 = 1u;
const CF_MESH: u32 = 2u;
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

struct CorridorIn {
    // Steps of CORRIDOR_STEP units about `middle`.
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
        return ground - BAND_SINK_M;
    }
    let prio = f32((word >> PRIO_SHIFT) & 7u);
    return ground + prio * BAND_PRIO_M - band_dive(ground, plane, abs(across) - edge) - select(0.0, BAND_UNDER_M, (flags & CF_UNDER) != 0u);
}

// A point at render-space `p` about the eye, moved toward the eye.
fn corridor_biased(p: vec3f) -> vec3f {
    let d = length(p);
    return p * max(0.0, 1.0 - (CORRIDOR_BIAS_M + CORRIDOR_BIAS_SHARE * d) / max(d, 1e-3));
}

fn across_of(place: u32) -> f32 {
    return f32(bitcast<i32>(place << 20u) >> 20u) * ACROSS_STEP_M;
}

fn half_of(word: u32) -> f32 {
    return f32(word >> HALF_SHIFT) * HALF_STEP_M;
}

// Units about the anchor of a vertex.
fn corridor_at(v: CorridorIn) -> vec2f {
    return vec2f(v.middle - surface.anchor) + vec2f(v.at) * CORRIDOR_STEP;
}

@vertex
fn corridor_vertex(v: CorridorIn) -> CorridorOut {
    let u = corridor_at(v);
    let s = lod_space(u);
    let flags = (v.word >> FLAGS_SHIFT) & 31u;
    let across = across_of(v.place);
    let half = half_of(v.word);
    let z = select(v.z, band_height(s, v.z, v.word, flags, across, half), (flags & (CF_SAMPLE | CF_MESH)) != 0u);
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: CorridorOut;
    out.clip = frame.view_proj * vec4f(corridor_biased(p), 1.0);
    // The crack and surface views show the terrain alone: past the near plane, the triangle clips.
    if (frame.debug == 3u || frame.debug == 4u) {
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
    var p = vec3f(to_render(lod_space(corridor_at(v))), v.z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    return frame.view_proj * vec4f(p, 1.0);
}

@fragment
fn corridor_fragment(in: CorridorOut) -> @location(0) vec4f {
    let mat = in.word & 127u;
    let su = lod_space(in.u);
    if (mat == MAT_GROUND) {
        return vec4f(ground_colour(su, in.relative, in.altitude, u32(ceil(lod(su)))), 1.0);
    }
    var n = normalize(cross(dpdx(in.relative), dpdy(in.relative)));
    n *= -sign(dot(n, in.relative));
    let region = region_at(su);
    let p = look.place.xy + in.relative.xy;
    let w = max(length(dpdx(in.relative)), length(dpdy(in.relative)));
    var albedo = rule(region, slot_kerb()).rgb * (0.94 + 0.12 * noise(p / 0.9));
    if (mat == MAT_SIDEWALK) {
        albedo = paving(region, p, w);
    } else if (mat >= LOOK_FIRST) {
        let across = abs(in.road.x);
        let along_w = max(abs(dpdx(in.along)), abs(dpdy(in.along))) * SLEEPER_SPACING_M;
        albedo = road_surface(region, (in.word >> CODE_SHIFT) & 15u, max(in.road.y - across, 0.0), across, in.road.y, in.along, p, w, w, along_w).rgb;
    }
    let lit = light_up(albedo, n, sun_share(sun_at(su), in.relative, n, LIFT_SOLID), 1.0, look.horizon.w);
    return vec4f(atmosphere(lit, in.relative, in.altitude), 1.0);
}
