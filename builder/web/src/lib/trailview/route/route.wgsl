// The planned route: a ribbon LIFT_M above the drawn surface through the points of route.ts, drawn after
// the terrain. Near, it lies on the ground BODY_M to each side, its edges draped on the surface; where
// lying would show it thinner than BODY_PIXELS it turns to face the eye. A crisp body with a lighter middle
// covers the ground, and a narrow glow beyond its edges adds light. Its depth moves toward the eye by
// PULL_M and PULL_PIXELS, so that the ground between two points does not hide it. It is not lit.
// A pin stands on a stem over a ring on the ground, all of a constant size on the screen: on top, a badge that
// faces the eye, or a sphere in the sun. It shows faintly through what hides it.
// #include ../terrain/drawn-mesh.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    pixel: f32,
}

// As POINT_WORDS of route.ts.
struct Point {
    at: vec2i,
    // sRGB bytes, red lowest.
    colour: u32,
}

// As PIN_WORDS of route.ts.
struct Pin {
    at: vec2i,
    // sRGB bytes, red lowest.
    colour: u32,
    // The layer of the badge plus one, or 0 for a sphere.
    badge: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> points: array<Point>;
@group(0) @binding(2) var<storage, read> pins: array<Pin>;
@group(0) @binding(3) var badges: texture_2d_array<f32>;
@group(0) @binding(4) var badge_sampler: sampler;

const LIFT_M: f32 = 0.3;
// The half-width of the body, in metres and at least in output pixels, and the width of the glow beyond it in output pixels.
const BODY_M: f32 = 1.0;
const BODY_PIXELS: f32 = 1.5;
const GLOW_PIXELS: f32 = 2.0;
// The body's light over the line's colour: LIGHTER more in the middle, DARKER less at the edge.
const BODY_GAIN: f32 = 1.2;
const LIGHTER: f32 = 0.6;
const DARKER: f32 = 0.35;
const OPACITY: f32 = 0.95;
const GLOW_GAIN: f32 = 0.8;
// The route fades in from FADE_NEAR_M to FADE_FAR_M from the eye, so that it does not cover the road under a walking eye.
const FADE_NEAR_M: f32 = 2.0;
const FADE_FAR_M: f32 = 12.0;
const PULL_M: f32 = 0.5;
const PULL_PIXELS: f32 = 4.0;
const PIN_PULL_M: f32 = 2.0;
// A pin in output pixels: the radius of its sphere, of its badge (half BADGE_SIZE of route.ts) and of its ring,
// the height of its stem and the stem's half-width.
const SPHERE_PIXELS: f32 = 11.0;
const BADGE_PIXELS: f32 = 16.0;
const RING_PIXELS: f32 = 16.0;
const STEM_PIXELS: f32 = 34.0;
const STEM_WIDTH_PIXELS: f32 = 1.75;
// The share of its light that a pin shows where something hides it: route.ts draws the pins again behind what is drawn.
override SHOWN: f32 = 1.0;

struct RouteOut {
    @builtin(position) clip: vec4f,
    // The side from −1 to 1, and the share of it that the body fills.
    @location(0) side: f32,
    @location(1) body: f32,
    @location(2) colour: vec3f,
    // 1 where the surface under the vertex is known.
    @location(3) kept: f32,
    @location(4) fade: f32,
}

fn centre(i: u32) -> vec2f {
    return lod_space(vec2f(points[i].at - surface.anchor));
}

// The ground under a point of LOD space.
fn ground(s: vec2f) -> f32 {
    return stand_height(s, surface.anchor + vec2i(round(units_of(s))));
}

fn direction(d: vec2f, fallback: vec2f) -> vec2f {
    return select(fallback, normalize(d), dot(d, d) > 1e-12);
}

// sRGB squared as linear light, as the colours of the terrain.
fn linear(srgb: vec3f) -> vec3f {
    return srgb * srgb;
}

// Moves p toward the eye by metres and pixels, keeping at least half its distance; its screen position stays.
fn pulled(p: vec3f, metres: f32, pixels: f32) -> vec3f {
    let reach = max(length(p), 1e-6);
    return p * max(reach - metres - pixels * frame.pixel * reach, 0.5 * reach) / reach;
}

fn eye_space(s: vec2f, h: f32) -> vec3f {
    let r = to_render(s);
    return vec3f(r, h + LIFT_M - frame.eye - frame.curvature * dot(r, r));
}

@vertex
fn route_vertex(@builtin(vertex_index) v: u32) -> RouteOut {
    let n = arrayLength(&points);
    let i = v >> 1u;
    let side = f32(v & 1u) * 2.0 - 1.0;
    let s = centre(i);
    let back = direction(s - centre(max(i, 1u) - 1u), vec2f(0.0, 1.0));
    let tangent = direction(back + direction(centre(min(i + 1u, n - 1u)) - s, back), back);
    let left = vec3f(-tangent.y, tangent.x, 0.0);
    let h = ground(s);
    let c = eye_space(s, select(frame.eye - LIFT_M - surface.above, h, known(h)));
    let distance = max(length(c), 1e-6);
    // Metres of one output pixel at the point.
    let metre = frame.pixel * distance;
    let body = max(BODY_M, BODY_PIXELS * metre);
    let half = body + GLOW_PIXELS * metre;
    // The half-width in pixels that the body shows when it lies on the ground.
    let lying_pixels = BODY_M * length(cross(left, c)) / (distance * metre);
    let lying = 1.0 - smoothstep(1.0, 2.0, BODY_PIXELS / max(lying_pixels, 1e-6));
    let foot = s + left.xy * side * half;
    let foot_h = ground(foot);
    let lie = eye_space(foot, select(h, foot_h, known(foot_h)));
    // Across the line as the eye sees it, on the side of left; a line that runs straight at the eye turns flat.
    let facing = cross(vec3f(tangent, 0.0), c) * select(1.0, -1.0, c.z > 0.0);
    let across = select(left, normalize(facing), dot(facing, facing) > 1e-6 * distance * distance);
    let p = pulled(mix(c + across * side * half, lie, lying), PULL_M, PULL_PIXELS);
    var out: RouteOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.side = side;
    out.body = body / half;
    out.colour = linear(unpack4x8unorm(points[i].colour).rgb);
    out.fade = smoothstep(FADE_NEAR_M, FADE_FAR_M, distance);
    // The crack and surface views show the terrain alone.
    out.kept = select(0.0, 1.0, known(h) && frame.debug != 3u && frame.debug != 4u);
    return out;
}

// Premultiplied: the body covers what lies behind it, the glow adds to it.
@fragment
fn route_fragment(in: RouteOut) -> @location(0) vec4f {
    let t = abs(in.side);
    let aa = max(fwidth(t), 1e-6);
    if (in.kept < 0.999) {
        discard;
    }
    let cover = clamp((in.body - t) / aa + 0.5, 0.0, 1.0);
    let inner = min(t / in.body, 1.0);
    let shade = BODY_GAIN * mix(1.0 + LIGHTER, 1.0 - DARKER, inner * inner);
    let outer = clamp((t - in.body) / max(1.0 - in.body, 1e-6), 0.0, 1.0);
    let glow = GLOW_GAIN * (1.0 - outer) * (1.0 - outer);
    let light = in.colour * (shade * cover * OPACITY + glow * (1.0 - cover));
    return vec4f(light, cover * OPACITY) * in.fade;
}

struct PinOut {
    @builtin(position) clip: vec4f,
    // −1 to 1 across the part.
    @location(0) uv: vec2f,
    // 0 the ring, 1 the stem, 2 the head.
    @location(1) @interpolate(flat) part: u32,
    @location(2) @interpolate(flat) pin: u32,
    // The axes of the head's image: right and up on the screen, and toward the eye.
    @location(3) @interpolate(flat) right: vec3f,
    @location(4) @interpolate(flat) up: vec3f,
    @location(5) @interpolate(flat) toward: vec3f,
    @location(6) kept: f32,
}

@vertex
fn pin_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) k: u32) -> PinOut {
    let corner = v % 6u;
    let uv = vec2f(f32((0x32u >> corner) & 1u), f32((0x2cu >> corner) & 1u)) * 2.0 - 1.0;
    let part = v / 6u;
    let s = lod_space(vec2f(pins[k].at - surface.anchor));
    let h = ground(s);
    let foot = eye_space(s, h);
    let distance = max(length(foot), 1e-6);
    let metre = frame.pixel * distance;
    let right = vec3f(direction(vec2f(foot.y, -foot.x), vec2f(1.0, 0.0)), 0.0);
    let up = normalize(cross(right, foot));
    let head = select(SPHERE_PIXELS, BADGE_PIXELS, pins[k].badge > 0u);
    var p = foot + vec3f(uv, 0.0) * RING_PIXELS * metre;
    if (part == 1u) {
        p = foot + right * uv.x * STEM_WIDTH_PIXELS * metre + vec3f(0.0, 0.0, (uv.y * 0.5 + 0.5) * STEM_PIXELS * metre);
    } else if (part == 2u) {
        p = foot + vec3f(0.0, 0.0, (STEM_PIXELS + head) * metre) + (right * uv.x + up * uv.y) * head * metre;
    }
    var out: PinOut;
    out.clip = frame.view_proj * vec4f(pulled(p, PIN_PULL_M, PULL_PIXELS), 1.0);
    out.uv = uv;
    out.part = part;
    out.pin = k;
    out.right = right;
    out.up = up;
    out.toward = -foot / distance;
    out.kept = select(0.0, 1.0, known(h) && frame.debug != 3u && frame.debug != 4u);
    return out;
}

// Premultiplied, as the route.
@fragment
fn pin_fragment(in: PinOut) -> @location(0) vec4f {
    let pin = pins[in.pin];
    let colour = linear(unpack4x8unorm(pin.colour).rgb);
    let r = length(in.uv);
    // From the middle to the edge: of the head and the ring by the radius, of the stem across it.
    let x = select(abs(in.uv.x), r, in.part != 1u);
    let aa = max(fwidth(x), 1e-4);
    let texel = textureSample(badges, badge_sampler, in.uv * vec2f(0.5, -0.5) + 0.5, max(pin.badge, 1u) - 1u);
    if (in.kept < 0.999) {
        discard;
    }
    if (in.part == 0u) {
        // A shadow that darkens the ground under a ring in the pin's colour.
        let shadow = 0.35 * (1.0 - smoothstep(0.0, 1.0, r));
        let ring = clamp((0.12 - abs(r - 0.82)) / aa + 0.5, 0.0, 1.0);
        return vec4f(colour * ring, max(shadow, ring)) * SHOWN;
    }
    if (in.part == 2u && pin.badge > 0u) {
        return vec4f(linear(texel.rgb / max(texel.a, 1e-6)) * texel.a, texel.a) * SHOWN;
    }
    // The stem and the sphere take the sun on their round sides; the sphere's edge turns from the eye and darkens.
    let across = select(vec2f(in.uv.x, 0.0), in.uv, in.part == 2u);
    let n = normalize(in.right * across.x + in.up * across.y + in.toward * sqrt(max(1.0 - x * x, 0.0)));
    let sun = max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w;
    let shine = pow(max(dot(reflect(-frame.sun.xyz, n), in.toward), 0.0), 20.0) * frame.sun.w;
    let base = select(vec3f(0.85), colour, in.part == 2u);
    let rim = mix(0.7, 1.0, dot(n, in.toward));
    let cover = clamp((1.0 - x) / aa + 0.5, 0.0, 1.0);
    return vec4f((base * (sun + 0.5) * rim + shine) * cover, cover) * SHOWN;
}
