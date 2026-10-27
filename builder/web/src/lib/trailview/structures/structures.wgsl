// The structures: the meshes of mesh.ts and the anchored meshes of anchored.ts, drawn before the terrain
// as opaque flat faces in their vertex colours. A vertex stands at its height from the bake, or on the
// drawn surface (plain_height), or hangs from an anchor. A road face takes the road paint of the ground,
// a stone face the colour of the rock under it. The light and the haze are those of the ground, from the
// look in group 2.
// #include ../terrain/drawn-mesh.wgsl
// #include ../ground/look.wgsl

// As Frame and Node of terrain.wgsl, which share their buffers.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    pixel: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;

// A span between two points of the drawn surface, as ANCHOR_WORDS of anchored.ts.
struct Anchor {
    a: vec2i,
    b: vec2i,
    // Metres over the drawn surface at a and b, and the sag at the middle.
    over: vec2f,
    sag: f32,
    flags: u32,
}

@group(0) @binding(1) var<storage, read> anchors: array<Anchor>;
// The heights of each anchor's ends above sea level: written by `anchor_heights`, read by `anchored_vertex`.
@group(0) @binding(2) var<storage, read_write> anchor_out: array<f32>;
@group(0) @binding(3) var<uniform> anchor_count: vec4u;
@group(0) @binding(4) var<storage, read> anchor_z: array<vec2f>;

struct StructureIn {
    @location(0) at: vec2i,
    @location(1) z: f32,
    // rgb: the colour in sRGB; for a road face, r is its surface code. a: the flags.
    @location(2) colour: vec4f,
    // A road face's metres across from its middle and its half-width, over ROAD_RANGE_M.
    @location(3) road: vec2f,
    // A track's sleepers from its start, NO_SLEEPERS elsewhere.
    @location(4) sleepers: f32,
}

struct StructureOut {
    @builtin(position) clip: vec4f,
    // Units about the anchor.
    @location(0) u: vec2f,
    @location(1) relative: vec3f,
    @location(2) altitude: f32,
    @location(3) @interpolate(flat) colour: vec4f,
    @location(4) road: vec2f,
    @location(5) sleepers: f32,
}

// Vertex flags, as in mesh.ts.
const ON_GROUND: u32 = 1u;
const ROAD_FACE: u32 = 2u;
const VOID_FACE: u32 = 4u;
const INSIDE_FACE: u32 = 8u;
const LAMP_FACE: u32 = 16u;
// As anchored.ts: a face in the colour of the rock under it; an edge of a cable's ribbon. An anchor that is a point.
const STONE_FACE: u32 = 32u;
const CABLE_EDGE: u32 = 64u;
const POINT_ANCHOR: u32 = 1u;
const ABSOLUTE_ANCHOR: u32 = 2u;
// The model grey that a stone face takes the rock's colour over: GREY of models/rocks.ts, squared as the albedo is.
const ROCK_GREY: vec3f = vec3f(0.3101, 0.3189, 0.2929);
// A cable's diameter, its least width on screen in output pixels (a render pixel at the default scale),
// the least share of that width that it covers, so that a far lift keeps a faint line, and the metres
// of an offset unit.
const CABLE_M: f32 = 0.05;
const CABLE_PIXELS: f32 = 1.5;
const CABLE_COVER: f32 = 0.4;
const OFFSET_M: f32 = 1.0 / 256.0;
// The light inside a tunnel: its lamps, the same everywhere.
const TUNNEL_LIGHT: vec3f = vec3f(0.36, 0.35, 0.33);
// The void of a portal draws past this share of the masks' reach: nearer, the terrain has its hole in
// the box and the bore shows through.
const VOID_REACH: f32 = 0.75;
// As ROAD_RANGE_M of mesh.ts.
const ROAD_RANGE_M: f32 = 16.0;

@vertex
fn structure_vertex(v: StructureIn) -> StructureOut {
    let u = vec2f(v.at - surface.anchor);
    let s = lod_space(u);
    let flags = u32(round(v.colour.a * 255.0));
    var out: StructureOut;
    var z = v.z;
    if ((flags & ON_GROUND) != 0u) {
        // Where no page knows the ground yet, the ground under the eye.
        let h = plain_height(s);
        z += select(frame.eye - surface.above, h, known(h));
    }
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    // The crack and surface views show the terrain alone: past the near plane, the triangle clips.
    if (frame.debug == 3u || frame.debug == 4u || ((flags & VOID_FACE) != 0u && length(p) < VOID_REACH * masks.reach)) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = u;
    out.relative = p;
    out.altitude = z;
    out.colour = v.colour;
    out.road = v.road * ROAD_RANGE_M;
    out.sleepers = v.sleepers;
    return out;
}

// The heights of the frame's anchors, one lane per end: the drawn surface there (`stand_height`), NONE
// where no page knows it. A point's far end takes the height of its near one.
@compute @workgroup_size(64)
fn anchor_heights(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x >> 1u;
    let end = id.x & 1u;
    if (i >= anchor_count.x) {
        return;
    }
    let an = anchors[i];
    if ((an.flags & ABSOLUTE_ANCHOR) != 0u) {
        anchor_out[id.x] = an.over[end];
        return;
    }
    let u = select(an.a, an.b, end == 1u && (an.flags & POINT_ANCHOR) == 0u);
    let h = stand_height(lod_space(vec2f(u - surface.anchor)), u);
    anchor_out[id.x] = select(NONE, h + an.over[end], known(h));
}

struct AnchoredIn {
    // Metres across, along and up in the anchor's frame, in OFFSET_M; w: the side of a cable's edge.
    @location(0) offset: vec4i,
    // The anchor in the low 16 bits, t in the high 16.
    @location(1) hung: u32,
    @location(2) colour: vec4f,
}

@vertex
fn anchored_vertex(v: AnchoredIn, @builtin(instance_index) first: u32) -> StructureOut {
    return anchored(v, first);
}

// A vertex of an anchored mesh in a shadow map (light/shadow.ts), whose frame holds the sun's view. A
// cable casts none: its ribbon is narrower than a texel.
@vertex
fn anchored_caster(v: AnchoredIn, @builtin(instance_index) first: u32) -> @builtin(position) vec4f {
    if ((u32(round(v.colour.a * 255.0)) & CABLE_EDGE) != 0u) {
        return vec4f(0.0, 0.0, 2.0, 1.0);
    }
    return anchored(v, first).clip;
}

fn anchored(v: AnchoredIn, first: u32) -> StructureOut {
    let i = first + (v.hung & 0xffffu);
    let t = f32(v.hung >> 16u) / 65535.0;
    let an = anchors[i];
    let z = anchor_z[i];
    let ua = vec2f(an.a - surface.anchor);
    let ub = vec2f(an.b - surface.anchor);
    let pa = to_render(lod_space(ua));
    let pb = to_render(lod_space(ub));
    let along = normalize(pb - pa);
    let right = vec2f(along.y, -along.x);
    let off = vec3f(v.offset.xyz) * OFFSET_M;
    let flags = u32(round(v.colour.a * 255.0));
    var p = vec3f(mix(pa, pb, t) + right * off.x + along * off.y, mix(z.x, z.y, t) - 4.0 * an.sag * t * (1.0 - t) + off.z - frame.eye);
    var out: StructureOut;
    out.road = vec2f(1.0, 0.0);
    out.sleepers = NO_SLEEPERS;
    if ((flags & CABLE_EDGE) != 0u) {
        // The ribbon turns to the eye and is at least CABLE_PIXELS wide; `road.x` keeps the share that the cable covers.
        let tangent = vec3f(pb - pa, z.y - z.x - 4.0 * an.sag * (1.0 - 2.0 * t));
        let width = max(CABLE_M, CABLE_PIXELS * length(p) * frame.pixel);
        p += normalize(cross(tangent, p)) * f32(v.offset.w) * 0.5 * width;
        out.road = vec2f(max(CABLE_M / width, CABLE_COVER), 0.0);
    }
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    // Where no page knows the height of an end yet, the vertex collapses.
    if (frame.debug == 3u || frame.debug == 4u || !known(z.x) || !known(z.y)) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    out.u = mix(ua, ub, t);
    out.relative = p;
    out.altitude = p.z + frame.eye;
    out.colour = v.colour;
    return out;
}

@fragment
fn structure_fragment(in: StructureOut) -> @location(0) vec4f {
    var n = normalize(cross(dpdx(in.relative), dpdy(in.relative)));
    n *= -sign(dot(n, in.relative));
    let flags = u32(round(in.colour.a * 255.0));
    var albedo = in.colour.rgb * in.colour.rgb;
    let w = max(length(dpdx(in.relative)), length(dpdy(in.relative)));
    // A pixel's span along a track: the rate of its count of sleepers.
    let along_w = max(abs(dpdx(in.sleepers)), abs(dpdy(in.sleepers))) * SLEEPER_SPACING_M;
    if ((flags & ROAD_FACE) != 0u) {
        let across = abs(in.road.x);
        albedo = road_surface(region_at(lod_space(in.u)), u32(round(in.colour.r * 255.0)), max(in.road.y - across, 0.0), across, in.road.y, false, in.sleepers, look.place.xy + in.relative.xy, w, w, along_w).rgb;
    }
    if ((flags & STONE_FACE) != 0u) {
        let su = lod_space(in.u);
        let cell = cell_at(su, u32(ceil(lod(su))));
        albedo *= rule(cell.b, slot_lithologies() + i32(cell.g)).rgb / ROCK_GREY;
    }
    // A cable narrower than its ribbon covers a share of it: the dither of that share resolves over the jittered frames.
    if ((flags & CABLE_EDGE) != 0u && hash(in.clip.xy + look.screen.xy * 4096.0) > in.road.x) {
        discard;
    }
    var lit = light_up(albedo, n, sun_share(sun_at(lod_space(in.u)), in.relative, n, LIFT_SOLID), 1.0, look.horizon.w);
    if ((flags & INSIDE_FACE) != 0u) {
        lit = albedo * TUNNEL_LIGHT;
    }
    if ((flags & (VOID_FACE | LAMP_FACE)) != 0u) {
        lit = albedo;
    }
    return vec4f(atmosphere(lit, in.relative, in.altitude), 1.0);
}
