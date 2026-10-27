// Pools of light about the lights of light-pools.ts, street lamps and fires, at night. After the frame, each light
// draws the back faces of a box about the reach of its light; a pixel finds its point from the depth (reversed Z:
// depth = NEAR / distance along the view) and its normal from the points next to it. The light falls off with the
// square of the distance and the slant of the face, as from a point, and fades to nothing at the light's radius. A
// lamp hangs in a housing, so the light falls below it and beside it, not above it. The mesh draws the ground in
// flat facets, which a low light would show: a face within a few degrees of the ground's slope at the light takes
// that slope.
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
    // The radius and the source's height over the ground, metres, as two float16.
    size: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> pools: array<Pool>;
@group(0) @binding(2) var depth: texture_depth_2d;
// xy: the render size in pixels.
@group(0) @binding(3) var<uniform> render_size: vec4f;

// As NEAR of frame/camera.ts.
const NEAR: f32 = 0.1;
// The light on the ground under the source over its colour.
const STRENGTH: f32 = 0.3;
// The light reaches this share of its radius above its source, and fades from below its height.
const RISE: f32 = 0.2;
// The cosines between a face and the ground's slope from which the face takes some of the slope, and all of it.
const FACET_COS = vec2f(0.94, 0.985);

struct PoolOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) source: vec3f,
    @location(1) @interpolate(flat) radius: f32,
    @location(2) @interpolate(flat) light: vec3f,
    @location(3) @interpolate(flat) ground: vec3f,
}

fn ground(s: vec2f) -> f32 {
    return stand_height(s, surface.anchor + vec2i(round(units_of(s))));
}

@vertex
fn pool_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) k: u32) -> PoolOut {
    // The corners of the 12 triangles of a box, as bits x, y, z; each face turns counterclockwise seen from outside.
    var corners = array<u32, 36>(0u, 2u, 3u, 0u, 3u, 1u, 4u, 5u, 7u, 4u, 7u, 6u, 0u, 1u, 5u, 0u, 5u, 4u,
                                 2u, 6u, 7u, 2u, 7u, 3u, 0u, 4u, 6u, 0u, 6u, 2u, 1u, 3u, 7u, 1u, 7u, 5u);
    let corner = corners[v];
    var out: PoolOut;
    // A light that sheds nothing, by day or off the drawn ground, draws no pixel.
    out.clip = vec4f(0.0, 0.0, 0.0, 1.0);
    if (look.ambient.w <= 0.0) {
        return out;
    }
    let pool = pools[k];
    let s = lod_space(vec2f(pool.at - surface.anchor));
    let h = ground(s);
    if (!known(h)) {
        return out;
    }
    let size = unpack2x16float(pool.size);
    var c = vec3f(to_render(s), h + size.y - frame.eye);
    c.z -= frame.curvature * dot(c.xy, c.xy);
    let box = vec3f(f32(corner & 1u), f32((corner >> 1u) & 1u), f32(corner >> 2u));
    out.clip = frame.view_proj * vec4f(c + vec3f((box.xy * 2.0 - 1.0) * size.x, mix(-size.x, RISE * size.x, box.z)), 1.0);
    out.source = c;
    out.radius = size.x;
    let srgb = unpack4x8unorm(pool.colour).rgb;
    // The ground under the source, size.y below it, takes STRENGTH.
    out.light = srgb * srgb * STRENGTH * (size.y * size.y + 1.0) * smoothstep(0.0, 0.4, look.ambient.w);
    out.ground = render_normal(drawn_normal(s));
    return out;
}

// The point of the frame at a pixel, in render space about the eye.
fn seen(pixel: vec2i) -> vec3f {
    let ndc = (vec2f(pixel) + 0.5) / render_size.xy * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    return (look.forward.xyz + ndc.x * look.right.xyz + ndc.y * look.up.xyz) * (NEAR / max(textureLoad(depth, pixel, 0), 1e-9));
}

// Of the steps to the two points next to a pixel on an axis, the shorter one: at the edge of a wall the
// step along the wall, not the one to what lies behind it. A step is zero at the border of the frame.
fn shorter(a: vec3f, b: vec3f) -> vec3f {
    return select(b, a, dot(a, a) > 0.0 && (dot(a, a) < dot(b, b) || dot(b, b) == 0.0));
}

// The normal of the frame at a pixel whose point is p, toward the eye.
fn normal_at(pixel: vec2i, p: vec3f) -> vec3f {
    let last = vec2i(render_size.xy) - 1;
    let across = shorter(seen(min(pixel + vec2i(1, 0), last)) - p, p - seen(max(pixel - vec2i(1, 0), vec2i(0))));
    let down = shorter(seen(min(pixel + vec2i(0, 1), last)) - p, p - seen(max(pixel - vec2i(0, 1), vec2i(0))));
    let n = normalize(cross(across, down));
    return select(n, -n, dot(n, p) > 0.0);
}

@fragment
fn pool_fragment(in: PoolOut) -> @location(0) vec4f {
    let pixel = vec2i(in.clip.xy);
    let p = seen(pixel);
    let to = in.source - p;
    let far = length(to);
    if (far >= in.radius) {
        discard;
    }
    let toward = to / far;
    let facet = normal_at(pixel, p);
    let n = normalize(mix(facet, in.ground, smoothstep(FACET_COS.x, FACET_COS.y, dot(facet, in.ground))));
    let shine = max(dot(n, toward), 0.0) * smoothstep(-RISE, 0.4, toward.z);
    let fade = 1.0 - pow(far / in.radius, 4.0);
    return vec4f(in.light * shine * fade * fade / (far * far + 1.0), 0.0);
}
