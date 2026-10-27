// The sky behind the terrain, in linear light: the sky colour, a halo about the light, the sun's disk
// about twice the real one, the moon in its phase, the Milky Way and the stars. One triangle covers the
// screen at depth 0, where the reversed-Z depth test leaves only the pixels that no terrain covers; the
// stars draw after it as small quads at depth 0. A port of the prototype's skyFragment without the clouds.
// #include shade.wgsl

struct Night {
    // J2000 equatorial directions to render space.
    stars: mat3x3f,
    // xyz: toward the moon. w: 1 while the moon is the light in the sun's place.
    moon: vec4f,
    // x: the faintest magnitude that shows. y: the share of the stars' light through the clouds. z: the
    // Milky Way's share. w: the moon disk's share, 0 under a full cloud cover.
    show: vec4f,
    // xy: clip space per render pixel. zw: unused.
    pixel: vec4f,
}

@group(0) @binding(0) var<uniform> night: Night;
// The Milky Way without its bright stars (tools/trailview/sky.py).
@group(0) @binding(1) var milky_way: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;

const TAU: f32 = 6.2831853;
// The half widths of the drawn disks in radians, twice the real ones.
const SUN_DISK: f32 = 0.0085;
const MOON_DISK: f32 = 0.0085;
// The light of a lit moon on the screen, and of its dark side in earthshine.
const MOON_LIGHT: f32 = 1.6;
const EARTHSHINE: f32 = 0.012;
// The Milky Way's light at its full share: its texels hold up to 1 in its core. The eye sees little
// colour at night, so the glow keeps this share of the image's colour.
const MILKY_WAY: f32 = 0.4;
const MILKY_WAY_COLOUR: f32 = 0.3;
// A star's light at magnitude 0, its brightness per magnitude (a flux ratio of 10^0.4 per magnitude
// would hide all but the brightest few), and its radius in output pixels: at least 1.5 so that the
// jittered frames keep it whole, more for the brightest.
const STAR_LIGHT: f32 = 10.0;
const STAR_STEP: f32 = 0.2;
const STAR_RADIUS: f32 = 1.5;

struct Sky {
    @builtin(position) clip: vec4f,
    @location(0) ndc: vec2f,
}

@vertex
fn sky_vertex(@builtin(vertex_index) i: u32) -> Sky {
    let ndc = vec2f(f32(i & 1u) * 4.0 - 1.0, f32(i >> 1u) * 4.0 - 1.0);
    return Sky(vec4f(ndc, 0.0, 1.0), ndc);
}

// The share of the light of the sky beyond the air toward a direction: 0.3 magnitudes per air mass.
fn clear(dir: vec3f) -> f32 {
    if (dir.z <= 0.0) {
        return 0.0;
    }
    let mass = 1.0 / (dir.z + 0.025 * exp(-11.0 * dir.z));
    return exp(-0.28 * (mass - 1.0));
}

// The moon over the sky colour `c` at a pixel's ray: a sphere lit from the sun, so its phase and the
// tilt of its lit side follow from the two directions.
fn moon(c: vec3f, dir: vec3f) -> vec3f {
    let m = night.moon.xyz;
    let across = (dir - m * dot(dir, m)) / MOON_DISK;
    let r = dot(across, across);
    if (r >= 1.0 || dot(dir, m) <= 0.0) {
        return c;
    }
    let normal = across - m * sqrt(1.0 - r);
    let lit = smoothstep(-0.04, 0.12, dot(normal, look.sky_sun.xyz)) + EARTHSHINE;
    let edge = smoothstep(1.0, 0.85, r);
    return c + vec3f(0.94, 0.95, 1.0) * MOON_LIGHT * lit * edge * night.show.w;
}

@fragment
fn sky_fragment(in: Sky) -> @location(0) vec4f {
    // The jitter moves the pixel, so the pixel shows the ray of its point without the jitter.
    let ndc = in.ndc - look.screen.xy;
    let dir = normalize(look.forward.xyz + ndc.x * look.right.xyz + ndc.y * look.up.xyz);
    // The halo about the light; the moon keeps only its core, as its wide halo would band in the dark sky.
    let near = max(dot(dir, look.sun.xyz), 0.0);
    var c = sky(dir) + look.sun_colour.rgb * (0.06 * (1.0 - night.moon.w) * pow(near, 16.0) + 0.35 * pow(near, 900.0));
    if (night.show.z > 0.0) {
        let s = transpose(night.stars) * dir;
        let uv = vec2f(0.5 - atan2(s.y, s.x) / TAU, 0.5 - asin(clamp(s.z, -1.0, 1.0)) / (0.5 * TAU));
        let glow = textureSampleLevel(milky_way, bilinear, uv, 0.0).rgb;
        c += mix(vec3f(luma(glow)), glow, MILKY_WAY_COLOUR) * (MILKY_WAY * night.show.z * clear(dir));
    }
    c = moon(c, dir);
    let disk = smoothstep(SUN_DISK + 0.001, SUN_DISK - 0.001, acos(clamp(dot(dir, look.sky_sun.xyz), -1.0, 1.0)));
    c = mix(c, look.sun_colour.rgb * 3.0 + 0.3, disk * (1.0 - night.moon.w) * step(0.001, dot(look.sun_colour.rgb, vec3f(1.0))));
    return vec4f(c, 1.0);
}

struct Star {
    @builtin(position) clip: vec4f,
    // The corner of the quad, -1 to 1.
    @location(0) corner: vec2f,
    @location(1) colour: vec3f,
}

// A star of light/stars.ts: its J2000 direction and V magnitude, and its colour.
@vertex
fn star_vertex(@builtin(vertex_index) i: u32, @location(0) star: vec4f, @location(1) tint: vec4f) -> Star {
    let dir = night.stars * star.xyz;
    let ahead = dot(dir, look.forward.xyz);
    let show = smoothstep(0.0, 1.0, night.show.x - star.w) * night.show.y * clear(dir);
    let corner = vec2f(f32(i & 1u), f32(i >> 1u)) * 2.0 - 1.0;
    if (ahead <= 0.01 || show <= 0.0) {
        return Star(vec4f(2.0, 2.0, 0.0, 1.0), corner, vec3f(0.0));
    }
    let right = look.right.xyz;
    let up = look.up.xyz;
    let at = vec2f(dot(dir, right) / dot(right, right), dot(dir, up) / dot(up, up)) / ahead + look.screen.xy;
    let radius = STAR_RADIUS * (1.0 + 0.5 * clamp(1.0 - star.w / 2.0, 0.0, 1.5));
    let light = STAR_LIGHT * pow(10.0, -STAR_STEP * star.w) * show / (radius * radius);
    return Star(vec4f(at + corner * radius * look.screen.z * night.pixel.xy, 0.0, 1.0), corner, tint.rgb * light);
}

@fragment
fn star_fragment(in: Star) -> @location(0) vec4f {
    return vec4f(in.colour * exp(-4.0 * dot(in.corner, in.corner)), 1.0);
}
