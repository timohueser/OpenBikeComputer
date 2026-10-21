// The water: the meshes of water-mesh.ts at their levels, one flat colour, drawn after the terrain
// and hidden where the terrain stands above them. Each drawn node draws the mesh of its feature tile
// and keeps the part over its own square, as its stamps read that tile. A skirt and a river edge
// move out by the reach of the node's level, under the banks that the stamps raise there. The haze
// and `finish()` are those of the ground, from the look in group 2.
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
}

struct Node {
    origin: vec2i,
    level: u32,
    unused: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(2) var<storage, read> nodes: array<Node>;
// Per instance, the index of its node in `nodes`.
@group(0) @binding(3) var<storage, read> water_nodes: array<u32>;

struct WaterIn {
    @location(0) at: vec2i,
    @location(1) level: f32,
    // The direction in which the vertex moves, with the length of a mitre, in world axes.
    @location(2) normal: vec2f,
    // It moves by `base` metres plus `reach` times the reach of the level: a river's half-width, a skirt.
    @location(3) base: f32,
    @location(4) reach: f32,
    // A river's half-width, 0 for a lake.
    @location(5) half: f32,
}

struct WaterOut {
    @builtin(position) clip: vec4f,
    // Units about the anchor, and the node's square.
    @location(0) u: vec2f,
    @location(1) @interpolate(flat) low: vec2f,
    @location(2) @interpolate(flat) high: vec2f,
    @location(3) relative: vec3f,
    @location(4) altitude: f32,
}

// The one colour of the water, in linear light.
const WATER: vec3f = vec3f(0.018, 0.071, 0.119);

@vertex
fn water_vertex(v: WaterIn, @builtin(instance_index) instance: u32) -> WaterOut {
    let node = nodes[water_nodes[instance]];
    let metres = unit_metres(v.at.y);
    let texel = metres * f32(1 << (22u - node.level));
    var out: WaterOut;
    // A river shows only where the stamps of the level carve its bed.
    if (v.half > 0.0 && !river_shows(v.half, node.level)) {
        out.clip = vec4f(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    // A skirt and a river's edge reach under the banks of the level, past its shore or its bed.
    let reach = water_reach(node.level, texel);
    let out_by = select(v.base + v.reach * reach, river_half(v.half, texel) + reach, v.half > 0.0);
    let u = vec2f(v.at - surface.anchor) + v.normal * out_by / metres;
    var p = vec3f(to_render(lod_space(u)), v.level - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.u = u;
    out.low = vec2f(node.origin);
    out.high = out.low + f32(1 << (30u - node.level));
    out.relative = p;
    out.altitude = v.level;
    return out;
}

@fragment
fn water_fragment(in: WaterOut) -> @location(0) vec4f {
    // The crack and surface views show the terrain alone.
    if (any(in.u < in.low) || any(in.u >= in.high) || frame.debug == 3u || frame.debug == 4u) {
        discard;
    }
    return vec4f(finish(atmosphere(WATER, in.relative, in.altitude)), 1.0);
}
