// The water: the meshes of water-mesh.ts, drawn after the terrain and hidden where the terrain stands
// above them. Each drawn node draws the mesh of the node whose page it draws and keeps the part over
// its own square. A lake lies at its level, a river area on the mesh without its snap (plain_height). A
// river or a stream is a ribbon on it at its width, at least RIBBON_PIXELS wide, and fades
// with its share of that width. The shading is the prototype's: a still body, dark from above and a
// mirror of the sky toward the horizon, with a sun glint; white where a ribbon runs down a steep bed.
// The haze is that of the ground, from the look in group 2; a ribbon blends in linear light.
// #include drawn-mesh.wgsl
// #include stamp-core.wgsl
// #include ../ground/look.wgsl

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
    // 1 where the height of the vertices is known.
    @location(8) known: f32,
    // How steeply a ribbon's bed falls along its flow.
    @location(9) fall: f32,
}

// The body of the water in linear light.
const BODY: vec3f = vec3f(0.025, 0.098, 0.175);
// A ribbon turns white where its bed falls more steeply than FOAM_LOW along its flow, up to FOAM_MOST at FOAM_HIGH.
const FOAM_LOW: f32 = 0.4;
const FOAM_HIGH: f32 = 1.0;
const FOAM_MOST: f32 = 0.7;
const RIBBON_PIXELS: f32 = 1.5;
// The level of a vertex that lies on the drawn surface, as DRAPED of water-mesh.ts.
const DRAPED: f32 = -1e30;

@vertex
fn water_vertex(v: WaterIn, @builtin(instance_index) instance: u32) -> WaterOut {
    let drawn = water_nodes[instance];
    let node = nodes[drawn.x];
    let metres = unit_metres(v.at.y);
    let lift = water_lift(metres * f32(1 << (22u - drawn.y)));
    var u = vec2f(v.at - surface.anchor);
    var out: WaterOut;
    out.share = 1.0;
    out.pixels = 1e6;
    if (v.half != 0.0) {
        let centre = vec3f(to_render(lod_space(u)), v.level - frame.eye);
        let pixel = frame.pixel * length(centre);
        let half = max(abs(v.half), 0.5 * RIBBON_PIXELS * pixel);
        u += v.across * half / metres;
        out.side = sign(v.half);
        out.share = abs(v.half) / half;
        out.pixels = half / pixel;
    }
    var z = v.level;
    if (v.half != 0.0 || v.level <= DRAPED) {
        z = plain_height(lod_space(u));
    }
    if (v.half != 0.0) {
        // At the middle of the ribbon, along the flow: across it, turned a right angle, in LOD space axes (y north).
        let n = drawn_normal(lod_space(vec2f(v.at - surface.anchor)));
        let flow = normalize(vec2f(v.across.y, v.across.x) + 1e-9);
        out.fall = abs(dot(n.xy, flow)) / n.z;
    }
    // A vertex of unknown height stands on the ground under the eye, as the terrain's does; the fragment drops its triangle.
    out.known = select(0.0, 1.0, known(z));
    z = select(frame.eye - surface.above, z, known(z)) + lift;
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
    return vec4f(atmosphere(water_colour(in), in.relative, in.altitude), in.share * edge);
}

fn water_colour(in: WaterOut) -> vec3f {
    let dir = normalize(in.relative);
    let world = in.relative.xy + look.place.xy;
    // Small waves tilt the surface by the slope of the noise; far away, where a pixel spans several, it lies calm.
    let calm = 1.0 - smoothstep(0.5, 4.0, frame.pixel * length(in.relative));
    let q = world / 1.7;
    let tilt = vec2f(noise(q + vec2f(0.35, 0.0)) - noise(q - vec2f(0.35, 0.0)), noise(q + vec2f(0.0, 0.35)) - noise(q - vec2f(0.0, 0.35))) * 0.06 * calm;
    let up = normalize(vec3f(2.0 * frame.curvature * in.relative.xy - tilt, 1.0));
    let mirrored = reflect(dir, up);
    // The reflection is held to 90 %: the far bank that it would show is darker than the sky.
    let fresnel = 0.02 + 0.98 * pow(clamp(1.0 - dot(-dir, up), 0.0, 1.0), 5.0);
    let lit = look.ambient.rgb + 0.8 * look.sun_colour.rgb * max(look.sun.z, 0.0);
    let body = BODY * lit;
    var c = mix(body * 0.55, sky(mirrored), min(0.96 * fresnel, 0.9));
    // The sun's path glints in sparks rather than a sheen.
    let glint = max(dot(mirrored, look.sun.xyz), 0.0);
    let sparks = smoothstep(0.8, 0.86, noise(q + 17.0)) * calm;
    c += look.sun_colour.rgb * (4.0 * pow(glint, 400.0) + 2.5 * sparks * pow(glint, 80.0));
    let foam = FOAM_MOST * smoothstep(FOAM_LOW, FOAM_HIGH, in.fall) * (0.6 + 0.4 * noise(world / 0.8));
    return mix(c, vec3f(0.85) * lit, foam);
}
