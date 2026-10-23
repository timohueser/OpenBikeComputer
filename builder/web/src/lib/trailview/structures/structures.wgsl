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
@group(0) @binding(2) var<storage, read_write> anchor_out: array<vec2f>;
@group(0) @binding(3) var<uniform> anchor_count: vec4u;
@group(0) @binding(4) var<storage, read> anchor_z: array<vec2f>;

struct StructureIn {
    @location(0) at: vec2i,
    @location(1) z: f32,
    // rgb: the colour in sRGB; for a road face, r is its surface code. a: the flags.
    @location(2) colour: vec4f,
    // A road face's metres across from its middle, and its half-width.
    @location(3) road: vec2f,
}

struct StructureOut {
    @builtin(position) clip: vec4f,
    // Units about the anchor.
    @location(0) u: vec2f,
    @location(1) relative: vec3f,
    @location(2) altitude: f32,
    @location(3) @interpolate(flat) colour: vec4f,
    @location(4) road: vec2f,
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

// The ground cell at a point, from the finest resident ground page: r the material, g the lithology, b the region.
fn cell_at(s: vec2f) -> vec4u {
    for (var k = min(u32(ceil(lod(s))), look.codes.y); k >= COARSEST; k--) {
        let t = texel_of(s, k);
        let whole = t.whole + vec2i(t.part >= vec2f(0.5));
        let page = ground_page(k, whole);
        if (page >= 0) {
            return textureLoad(cells, in_page(page, whole & vec2i(255)), page_layer(page), 0);
        }
    }
    return vec4u(0u);
}

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
    out.road = v.road;
    return out;
}

// The heights of the frame's anchors: the drawn surface at each end, or under the eye where no page knows it.
@compute @workgroup_size(64)
fn anchor_heights(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= anchor_count.x) {
        return;
    }
    let an = anchors[id.x];
    let fallback = frame.eye - surface.above;
    let ha = plain_height(lod_space(vec2f(an.a - surface.anchor)));
    var z = vec2f(select(fallback, ha, known(ha)));
    if ((an.flags & POINT_ANCHOR) == 0u) {
        let hb = plain_height(lod_space(vec2f(an.b - surface.anchor)));
        z.y = select(fallback, hb, known(hb));
    }
    anchor_out[id.x] = z + an.over;
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
    if ((flags & CABLE_EDGE) != 0u) {
        // The ribbon turns to the eye and is at least CABLE_PIXELS wide; `road.x` keeps the share that the cable covers.
        let tangent = vec3f(pb - pa, z.y - z.x - 4.0 * an.sag * (1.0 - 2.0 * t));
        let width = max(CABLE_M, CABLE_PIXELS * length(p) * frame.pixel);
        p += normalize(cross(tangent, p)) * f32(v.offset.w) * 0.5 * width;
        out.road = vec2f(max(CABLE_M / width, CABLE_COVER), 0.0);
    }
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    if (frame.debug == 3u || frame.debug == 4u) {
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
    if ((flags & ROAD_FACE) != 0u) {
        let across = abs(in.road.x);
        albedo = road_surface(cell_at(lod_space(in.u)).b, u32(round(in.colour.r * 255.0)), max(in.road.y - across, 0.0), across, in.road.y, look.place.xy + in.relative.xy, w, w).rgb;
    }
    if ((flags & STONE_FACE) != 0u) {
        let cell = cell_at(lod_space(in.u));
        albedo *= rule(cell.b, slot_lithologies() + i32(cell.g)).rgb / ROCK_GREY;
    }
    // A cable narrower than its ribbon covers a share of it: the dither of that share resolves over the jittered frames.
    if ((flags & CABLE_EDGE) != 0u && hash(in.clip.xy + look.screen.xy * 4096.0) > in.road.x) {
        discard;
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
