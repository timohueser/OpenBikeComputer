// The water: the meshes of water-mesh.ts, one flat colour, drawn after the terrain and hidden where
// the terrain stands above them. Each drawn node draws the mesh of the node whose stamps it draws and
// keeps the part over its own square. A lake lies at its level; a river or a stream is a ribbon on the
// drawn surface at its width, at least RIBBON_PIXELS wide, and fades with its share of that width. The
// haze and `finish()` are those of the ground, from the look in group 2.
// #include drawn-height.wgsl
// #include stamp-core.wgsl
// #include ../light/shade.wgsl

// As Frame and Node of terrain.wgsl, which share their buffers.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    // Metres of a pixel per metre of distance.
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
@group(0) @binding(3) var<storage, read> water_nodes: array<vec2u>;

struct WaterIn {
    @location(0) at: vec2i,
    @location(1) level: f32,
    // A ribbon's direction across its line, with the length of a mitre, in world axes; 0 for a lake.
    @location(2) across: vec2f,
    // A ribbon's half-width in metres, negative on its right side; 0 for a lake.
    @location(3) half: f32,
}

struct WaterOut {
    @builtin(position) clip: vec4f,
    // Units about the anchor, and the node's square.
    @location(0) u: vec2f,
    @location(1) @interpolate(flat) low: vec2f,
    @location(2) @interpolate(flat) high: vec2f,
    @location(3) relative: vec3f,
    @location(4) altitude: f32,
    // A ribbon's side from −1 to 1, its share of the drawn width and its drawn half-width in pixels.
    @location(5) side: f32,
    @location(6) share: f32,
    @location(7) pixels: f32,
    // 1 where the surface under a ribbon is known.
    @location(8) known: f32,
}

// The one colour of the water, in linear light.
const WATER: vec3f = vec3f(0.018, 0.071, 0.119);
const RIBBON_PIXELS: f32 = 1.5;
// Water lies LIFT_M plus LIFT of a mesh cell above the terrain that it meets, so that the mesh between
// the vertices of a ribbon, and the terrain at a lake's shore, stay under it.
const LIFT_M: f32 = 0.03;
const LIFT: f32 = 0.02;

@vertex
fn water_vertex(v: WaterIn, @builtin(instance_index) instance: u32) -> WaterOut {
    let drawn = water_nodes[instance];
    let node = nodes[drawn.x];
    let metres = unit_metres(v.at.y);
    let lift = LIFT_M + LIFT * MESH_TEXELS * metres * f32(1 << (22u - drawn.y));
    var u = vec2f(v.at - surface.anchor);
    var z = v.level + lift;
    var out: WaterOut;
    out.share = 1.0;
    out.pixels = 1e6;
    out.known = 1.0;
    if (v.half != 0.0) {
        let centre = vec3f(to_render(lod_space(u)), v.level - frame.eye);
        let pixel = frame.pixel * length(centre);
        let half = max(abs(v.half), 0.5 * RIBBON_PIXELS * pixel);
        u += v.across * half / metres;
        let h = drawn_height(lod_space(u));
        z = select(v.level, h, known(h)) + lift;
        out.side = sign(v.half);
        out.share = abs(v.half) / half;
        out.pixels = half / pixel;
        out.known = select(0.0, 1.0, known(h));
    }
    var p = vec3f(to_render(lod_space(u)), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.u = u;
    out.low = vec2f(node.origin);
    out.high = out.low + f32(1 << (30u - node.level));
    out.relative = p;
    out.altitude = z;
    return out;
}

@fragment
fn water_fragment(in: WaterOut) -> @location(0) vec4f {
    // The crack and surface views show the terrain alone.
    if (any(in.u < in.low) || any(in.u >= in.high) || in.known < 0.999 || frame.debug == 3u || frame.debug == 4u) {
        discard;
    }
    let edge = clamp((1.0 - abs(in.side)) * in.pixels + 0.5, 0.0, 1.0);
    return vec4f(finish(atmosphere(WATER, in.relative, in.altitude)), in.share * edge);
}
