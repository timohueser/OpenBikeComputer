// Pools of light about the lights of light-pools.ts, street lamps and fires, at night. After the frame, each light
// draws a square that faces the eye over its sphere, or the whole screen when the eye is in it; a pixel finds its
// point from the depth (reversed Z: depth = NEAR / distance along the view) and takes light by its distance from
// the light's source, falling to nothing at the light's radius. So the light falls evenly on what lies there.
// #include ../terrain/drawn-mesh.wgsl
// #include ../light/shade.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
}

// As POOL_WORDS of light-pools.ts.
struct Pool {
    at: vec2i,
    // sRGB bytes, red lowest.
    colour: u32,
    radius: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> pools: array<Pool>;
@group(0) @binding(2) var depth: texture_depth_2d;
// xy: the render size in pixels.
@group(0) @binding(3) var<uniform> render_size: vec4f;

// As NEAR of frame/camera.ts.
const NEAR: f32 = 0.1;
// The source of a light stands this high over the ground at its point.
const SOURCE_M: f32 = 1.5;
// The light at the source over its colour.
const STRENGTH: f32 = 0.22;

struct PoolOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) source: vec3f,
    @location(1) @interpolate(flat) radius: f32,
    @location(2) @interpolate(flat) light: vec3f,
}

fn ground(s: vec2f) -> f32 {
    return stand_height(s, surface.anchor + vec2i(round(units_of(s))));
}

@vertex
fn pool_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) k: u32) -> PoolOut {
    let pool = pools[k];
    let corner = vec2f(f32((0x32u >> v) & 1u), f32((0x2cu >> v) & 1u)) * 2.0 - 1.0;
    let s = lod_space(vec2f(pool.at - surface.anchor));
    let h = ground(s);
    var c = vec3f(to_render(s), h + SOURCE_M - frame.eye);
    c.z -= frame.curvature * dot(c.xy, c.xy);
    var out: PoolOut;
    // A sphere seen from at least 2 radii fits in a square 2 radii to each side about its middle, also
    // toward the edge of the view, where the square, which faces along the view, cuts it at a slant.
    let square = c + (normalize(look.right.xyz) * corner.x + normalize(look.up.xyz) * corner.y) * 2.0 * pool.radius;
    out.clip = select(frame.view_proj * vec4f(square, 1.0), vec4f(corner, 0.5, 1.0), length(c) < 2.0 * pool.radius);
    out.source = c;
    out.radius = pool.radius;
    let srgb = unpack4x8unorm(pool.colour).rgb;
    out.light = srgb * srgb * STRENGTH * smoothstep(0.0, 0.4, look.ambient.w) * select(0.0, 1.0, known(h));
    return out;
}

@fragment
fn pool_fragment(in: PoolOut) -> @location(0) vec4f {
    let d = textureLoad(depth, vec2i(in.clip.xy), 0);
    let ndc = vec2f(in.clip.x / render_size.x * 2.0 - 1.0, 1.0 - in.clip.y / render_size.y * 2.0);
    let p = (look.forward.xyz + ndc.x * look.right.xyz + ndc.y * look.up.xyz) * (NEAR / max(d, 1e-9));
    let t = length(p - in.source) / in.radius;
    if (d <= 0.0 || t >= 1.0) {
        discard;
    }
    let fall = 1.0 - t * t;
    return vec4f(in.light * fall * fall, 0.0);
}
