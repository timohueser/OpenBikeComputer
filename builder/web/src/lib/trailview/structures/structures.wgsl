// The structures: the meshes of mesh.ts, drawn before the terrain as opaque flat faces in their vertex
// colours. A vertex stands at its height from the bake, or on the drawn surface (plain_height). A
// road face takes the road paint of the ground. The light and the haze are those of the ground, from
// the look in group 2.
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
        albedo = road_surface(region_at(lod_space(in.u)), u32(round(in.colour.r * 255.0)), max(in.road.y - across, 0.0), across, in.road.y, in.sleepers, look.place.xy + in.relative.xy, w, w, along_w).rgb;
    }
    var lit = light_up(albedo, n, 1.0, 1.0, look.horizon.w);
    if ((flags & INSIDE_FACE) != 0u) {
        lit = albedo * TUNNEL_LIGHT;
    }
    if ((flags & (VOID_FACE | LAMP_FACE)) != 0u) {
        lit = albedo;
    }
    return vec4f(atmosphere(lit, in.relative, in.altitude), 1.0);
}
