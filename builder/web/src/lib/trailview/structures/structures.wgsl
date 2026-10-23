// The structures: the meshes of mesh.ts, drawn after the terrain as opaque flat faces in their vertex
// colours. Each drawn node draws the mesh of the node whose page it draws and keeps the part over its
// own square. A vertex stands at its height from the bake, or on the drawn surface (plain_height). A
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

struct Node {
    origin: vec2i,
    level: u32,
    unused: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(2) var<storage, read> nodes: array<Node>;
// Per instance, the index of its node in `nodes` and the level of the node that the mesh belongs to.
@group(0) @binding(3) var<storage, read> structure_nodes: array<vec2u>;

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
    // Units about the anchor, and the node's square.
    @location(0) u: vec2f,
    @location(1) @interpolate(flat) low: vec2f,
    @location(2) @interpolate(flat) high: vec2f,
    @location(3) relative: vec3f,
    @location(4) altitude: f32,
    @location(5) @interpolate(flat) colour: vec4f,
    @location(6) road: vec2f,
    // 1 where the height of the vertices is known.
    @location(7) known: f32,
}

// Vertex flags, as in mesh.ts.
const ON_GROUND: u32 = 1u;
const ROAD_FACE: u32 = 2u;
const VOID_FACE: u32 = 4u;

// The region of the ground at a point, from the finest resident ground page.
fn region_at(s: vec2f) -> u32 {
    for (var k = min(u32(ceil(lod(s))), look.codes.y); k >= COARSEST; k--) {
        let t = texel_of(s, k);
        let whole = t.whole + vec2i(t.part >= vec2f(0.5));
        let page = ground_page(k, whole);
        if (page >= 0) {
            return textureLoad(cells, in_page(page, whole & vec2i(255)), page_layer(page), 0).b;
        }
    }
    return 0u;
}

@vertex
fn structure_vertex(v: StructureIn, @builtin(instance_index) instance: u32) -> StructureOut {
    let drawn = structure_nodes[instance];
    let node = nodes[drawn.x];
    let u = vec2f(v.at - surface.anchor);
    let s = lod_space(u);
    let flags = u32(round(v.colour.a * 255.0));
    var out: StructureOut;
    var z = v.z;
    out.known = 1.0;
    if ((flags & ON_GROUND) != 0u) {
        let h = plain_height(s);
        // A vertex of unknown height stands on the ground under the eye; the fragment drops its triangle.
        out.known = select(0.0, 1.0, known(h));
        z += select(frame.eye - surface.above, h, known(h));
    }
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.u = u;
    out.low = vec2f(node.origin);
    out.high = out.low + f32(1 << (30u - node.level));
    out.relative = p;
    out.altitude = z;
    out.colour = v.colour;
    out.road = v.road;
    return out;
}

@fragment
fn structure_fragment(in: StructureOut) -> @location(0) vec4f {
    // The crack and surface views show the terrain alone.
    if (any(in.u < in.low) || any(in.u >= in.high) || in.known < 0.999 || frame.debug == 3u || frame.debug == 4u) {
        discard;
    }
    var n = normalize(cross(dpdx(in.relative), dpdy(in.relative)));
    n *= -sign(dot(n, in.relative));
    let flags = u32(round(in.colour.a * 255.0));
    var albedo = in.colour.rgb * in.colour.rgb;
    let w = max(length(dpdx(in.relative)), length(dpdy(in.relative)));
    if ((flags & ROAD_FACE) != 0u) {
        let across = abs(in.road.x);
        albedo = road_surface(region_at(lod_space(in.u)), u32(round(in.colour.r * 255.0)), max(in.road.y - across, 0.0), across, in.road.y, look.place.xy + in.relative.xy, w, w).rgb;
    }
    let lit = select(light_up(albedo, n, 1.0, 1.0, look.horizon.w), albedo, (flags & VOID_FACE) != 0u);
    return vec4f(atmosphere(lit, in.relative, in.altitude), 1.0);
}
