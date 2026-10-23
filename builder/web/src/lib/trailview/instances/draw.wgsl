// Draws the instances of one draw: its model's mesh of that tier, once per instance of its list, in the
// vertex colours under the light and the haze of the ground. Two tiers that fade into each other keep
// complementary pixels of a dither that changes with each jittered frame, so the resolve blends them.
// #include ../light/shade.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    pixel: f32,
}

struct Instance {
    at: vec4f,
    turn: u32,
    model: u32,
    look: u32,
    draw: u32,
}

struct Model {
    until: vec4f,
    extent: f32,
    radius: f32,
    fade: f32,
    // The farthest that a vertex lies from the model's axis, in model units: a crown's radius.
    spread: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> lists: array<Instance>;
@group(0) @binding(2) var<storage, read> bases: array<u32>;
@group(0) @binding(3) var<uniform> models: array<Model, 128>;
// The draw of this draw call.
@group(0) @binding(4) var<uniform> draw: vec4u;

const INCOMING: u32 = 1u << 16u;

struct MeshIn {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    @location(2) colour: vec4f,
}

struct MeshOut {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) albedo: vec3f,
    @location(2) @interpolate(flat) look: u32,
    // The haze over the instance: the light that reaches the eye is fog × lit + air.
    @location(3) fog: vec3f,
    @location(4) air: vec3f,
}

// The haze between the eye and a point of render space, as the factor and the term that `atmosphere`
// applies to a colour: per vertex, as an instance is small against the haze's scale.
struct Haze {
    fog: vec3f,
    air: vec3f,
}

fn haze_at(p: vec3f) -> Haze {
    let air = atmosphere(vec3f(0.0), p, frame.eye + p.z);
    return Haze(atmosphere(vec3f(1.0), p, frame.eye + p.z) - air, air);
}

fn linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

@vertex
fn mesh_vertex(v: MeshIn, @builtin(instance_index) instance: u32) -> MeshOut {
    let i = lists[bases[draw.x] + instance];
    let turn = unpack2x16float(i.turn);
    let local = v.position.xyz * models[i.model].extent;
    let p = i.at.xyz + i.at.w * vec3f(turn.x * local.x - turn.y * local.y, turn.y * local.x + turn.x * local.y, local.z);
    let t = normalize(turn);
    var out: MeshOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    let haze = haze_at(p);
    out.fog = haze.fog;
    out.air = haze.air;
    out.normal = vec3f(t.x * v.normal.x - t.y * v.normal.y, t.y * v.normal.x + t.x * v.normal.y, v.normal.z);
    out.albedo = linear(v.colour.rgb) * (0.75 + 0.5 * f32(i.look & 255u) / 255.0);
    out.look = i.look;
    return out;
}

// A dither value of a pixel in [0, 1): interleaved gradient noise, turned by the golden ratio with
// each jittered frame, so the frames of a still view take evenly spread values and their mean is smooth.
fn dither(pixel: vec2f) -> f32 {
    let ign = fract(52.9829189 * fract(dot(floor(pixel), vec2f(0.06711056, 0.00583715))));
    return fract(ign + 0.6180340 * f32(look.modes.y));
}

// Whether a pixel of an instance draws, by its dither: the share `appear` of its pixels, and in a fade
// between two tiers, complementary shares.
fn kept(pixel: vec2f, look: u32) -> bool {
    let appear = f32(look >> 24u) / 255.0;
    let x = f32((look >> 8u) & 255u) / 255.0;
    let fading = dither(pixel);
    let incoming = (look & INCOMING) != 0u;
    return dither(pixel + vec2f(37.0, 11.0)) < appear && select(fading >= x, fading < x, incoming);
}

@fragment
fn mesh_fragment(in: MeshOut) -> @location(0) vec4f {
    if (!kept(in.clip.xy, in.look)) {
        discard;
    }
    let n = normalize(in.normal);
    return vec4f(in.fog * light_up(in.albedo, n, 1.0, 1.0, look.horizon.w) + in.air, 1.0);
}
