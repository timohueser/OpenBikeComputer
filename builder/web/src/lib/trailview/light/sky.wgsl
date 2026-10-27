// The sky behind the terrain, in linear light: the sky colour, a halo about the light, the sun's disk
// about twice the real one, the moon in its phase, a solar eclipse's corona, the Milky Way and the stars. One triangle covers the
// screen at depth 0, where the reversed-Z depth test leaves only the pixels that no terrain covers; the
// stars draw after it as small quads at depth 0. A port of the prototype's skyFragment without the clouds.
// #include shade.wgsl

struct Night {
    // J2000 equatorial directions to render space.
    stars: mat3x3f,
    // xyz: toward the moon as drawn (light/eclipse.ts). w: 1 while the moon is the light in the sun's place.
    moon: vec4f,
    // x: the faintest magnitude that shows. y: the share of the stars' light through the clouds. z: the
    // Milky Way's share. w: the moon disk's share, 0 under a full cloud cover.
    show: vec4f,
    // xy: clip space per render pixel. z: the drawn sun's half width, w: the drawn moon's, in radians.
    pixel: vec4f,
    // rgb: the colour that the air gives the moon's disk at its altitude, as it gives the sun's.
    moon_air: vec4f,
    // A solar eclipse. x: the corona's share, 0 without one. y: the share of the sun's light that the
    // moon covers. z: the sun's activity, 0 at a minimum of its cycle to 1 at a maximum. w: the seed of
    // the corona's streamers.
    eclipse: vec4f,
}

@group(0) @binding(0) var<uniform> night: Night;
// The Milky Way without its bright stars (tools/trailview/sky.py).
@group(0) @binding(1) var milky_way: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
// The near side of the moon, an orthographic disk with lunar north up (tools/trailview/sky.py).
@group(0) @binding(3) var moon_face: texture_2d<f32>;

const TAU: f32 = 6.2831853;
// The pole of the ecliptic in J2000 equatorial directions: the obliquity is 23.44°.
const ECLIPTIC_POLE = vec3f(0.0, -0.3977772, 0.9174821);
// The edge of the drawn sun and moon in radians, about half a render pixel.
const EDGE: f32 = 0.001;
// The light of a lit moon on the screen, and of its dark side in earthshine. `moon_face` holds its
// albedo with this mean.
const MOON_LIGHT: f32 = 1.6;
const MOON_MEAN: f32 = 0.4;
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

// The share of a ray that the drawn moon covers.
fn behind_moon(dir: vec3f) -> f32 {
    let m = night.moon.xyz;
    return smoothstep(night.pixel.w + EDGE, night.pixel.w - EDGE, length(dir - m * dot(dir, m))) * step(0.0, dot(dir, m));
}

// The corona's light at the sun's limb in totality, the share of it in the fine rays, and the
// streamers of the sun's helmet: their number and the light of the brightest.
const CORONA: f32 = 2.5;
const RAYS: f32 = 0.25;
const STREAMERS: i32 = 9;
const STREAMER: f32 = 0.3;
// The prominences glow in the red light of hydrogen; they draw far higher than they are, or they
// would stay below a pixel. The diamond ring's light in the sun's, and its core and glow in radians.
const BEAD: f32 = 30.0;
const BEAD_CORE: f32 = 0.003;
const BEAD_GLOW: f32 = 0.02;
const PROMINENCE = vec3f(2.0, 0.35, 0.65);

// The corona about the sun at a ray, without the moon in front: the inner corona falls off steeply
// from the limb, helmet streamers reach out a few radii and narrow as they go, and fine rays run
// outward. At a minimum of the sun's cycle the streamers keep to its equator, and plumes fan out of its
// dimmer poles. At the limb the chromosphere and the prominences show where the moon leaves them.
fn corona(dir: vec3f) -> vec3f {
    let s = look.sky_sun.xyz;
    let at = (dir - s * dot(dir, s)) / night.pixel.z;
    let rho = length(at);
    if (rho < 0.9 || rho > 10.0 || dot(dir, s) <= 0.0) {
        return vec3f(0.0);
    }
    // The sun's axis stays within 7.25° of the pole of the ecliptic. θ turns from the sun's north.
    let pole = night.stars * ECLIPTIC_POLE;
    let north = normalize(pole - s * dot(pole, s));
    let theta = atan2(dot(at, cross(s, north)), dot(at, north));
    let quiet = 1.0 - night.eclipse.z;
    let seed = night.eclipse.w;
    let polar = abs(cos(theta));
    var streamers = 0.0;
    for (var k = 0; k < STREAMERS; k++) {
        let a = fract(seed * 7.31 + f32(k) * 0.618034);
        let b = fract(seed * 3.17 + f32(k) * 0.414214);
        // Near the equator at a minimum, anywhere at a maximum.
        let equator = select(-1.0, 1.0, (k & 1) == 0) * (0.25 + 0.2 * (a - 0.5)) * TAU;
        let centre = mix((a * 2.0 - 1.0) * 0.5 * TAU, equator, quiet);
        let off = atan2(sin(theta - centre), cos(theta - centre));
        let width = (0.2 + 0.35 * b) / sqrt(rho);
        streamers += (0.4 + 0.6 * b) * exp(-off * off / (width * width));
    }
    let rays = 1.0 + RAYS * sin(53.0 * theta + 40.0 * seed) * sin(31.0 * theta + 17.0 * seed);
    let plumes = 1.0 + 0.8 * quiet * pow(polar, 6.0) * max(sin(71.0 * theta + 9.0 * seed), 0.0);
    let inner = pow(rho, -4.5) * (1.0 - 0.6 * quiet * polar * polar) * plumes + 0.02 * pow(rho, -2.0);
    let light = CORONA * smoothstep(0.98, 1.02, rho) * rays * (inner + STREAMER * streamers * pow(rho, -2.2));
    let white = mix(vec3f(1.0, 0.93, 0.84), vec3f(0.9, 0.95, 1.0), smoothstep(1.0, 3.0, rho));
    let height = 0.02 + 0.3 * pow(max(sin(5.0 * theta + 11.0 * seed) * sin(8.0 * theta + 23.0 * seed), 0.0), 2.0);
    let limb = smoothstep(0.96, 1.0, rho) * smoothstep(1.0 + height, 1.0 + 0.5 * height, rho);
    return white * light + PROMINENCE * limb;
}

// The moon over the sky colour `c` at a pixel's ray: a sphere lit from the sun, so its phase and the
// tilt of its lit side follow from the two directions. Its face turns lunar north toward the pole of
// the ecliptic, which lunar north stays within 1.5° of.
fn moon(c: vec3f, dir: vec3f) -> vec3f {
    let m = night.moon.xyz;
    let across = (dir - m * dot(dir, m)) / night.pixel.w;
    let r = dot(across, across);
    if (r >= 1.0 || dot(dir, m) <= 0.0) {
        return c;
    }
    let pole = night.stars * ECLIPTIC_POLE;
    let north = normalize(pole - m * dot(pole, m));
    let uv = vec2f(0.5 + 0.5 * dot(across, cross(m, north)), 0.5 - 0.5 * dot(across, north));
    let albedo = textureSampleLevel(moon_face, bilinear, uv, 0.0).rgb / MOON_MEAN;
    let normal = across - m * sqrt(1.0 - r);
    let lit = smoothstep(-0.04, 0.12, dot(normal, look.sky_sun.xyz)) + EARTHSHINE;
    let edge = smoothstep(1.0, 0.9, r);
    return c + albedo * night.moon_air.rgb * MOON_LIGHT * lit * edge * night.show.w;
}

@fragment
fn sky_fragment(in: Sky) -> @location(0) vec4f {
    // The jitter moves the pixel, so the pixel shows the ray of its point without the jitter.
    let ndc = in.ndc - look.screen.xy;
    let dir = normalize(look.forward.xyz + ndc.x * look.right.xyz + ndc.y * look.up.xyz);
    // The halo about the light; the moon keeps only its core, as its wide halo would band in the dark sky.
    // In an eclipse it follows the sun's light that the moon leaves: the centre of the crescent, which
    // nears the limb as the crescent thins and is the diamond ring at its end.
    let s = look.sky_sun.xyz;
    let covered = night.eclipse.y;
    let away = -(night.moon.xyz - s * dot(night.moon.xyz, s)) * 0.5 * covered / max(1.0 - covered, 1e-4);
    let glare = normalize(look.sun.xyz + away * min(1.0, night.pixel.z / max(length(away), 1e-9)));
    let near = max(dot(dir, glare), 0.0);
    var c = sky(dir) + look.sun_colour.rgb * (0.06 * (1.0 - night.moon.w) * pow(near, 16.0) + 0.35 * pow(near, 900.0));
    if (night.show.z > 0.0) {
        let s = transpose(night.stars) * dir;
        let uv = vec2f(0.5 - atan2(s.y, s.x) / TAU, 0.5 - asin(clamp(s.z, -1.0, 1.0)) / (0.5 * TAU));
        let glow = textureSampleLevel(milky_way, bilinear, uv, 0.0).rgb;
        c += mix(vec3f(luma(glow)), glow, MILKY_WAY_COLOUR) * (MILKY_WAY * night.show.z * clear(dir));
    }
    // The moon hides the sun and the corona; the air in front of it keeps its light.
    let hidden = behind_moon(dir);
    if (night.eclipse.x > 0.0) {
        c += corona(dir) * (night.eclipse.x * (1.0 - hidden));
    }
    c = moon(c, dir);
    let disk = smoothstep(night.pixel.z + EDGE, night.pixel.z - EDGE, length(dir - s * dot(dir, s))) * step(0.0, dot(dir, s)) * (1.0 - hidden);
    c = mix(c, look.sun_colour.rgb * 3.0 + 0.3, disk * (1.0 - night.moon.w) * step(0.001, dot(look.sun_colour.rgb, vec3f(1.0))));
    // The diamond ring: as the corona shows, the last sliver of the sun glares as a point at the limb,
    // until totality puts it out.
    let bead = length(dir - glare * dot(dir, glare));
    c += look.sun_colour.rgb * (night.eclipse.x * BEAD) * (exp(-bead * bead / (BEAD_CORE * BEAD_CORE)) + 0.15 * exp(-bead * bead / (BEAD_GLOW * BEAD_GLOW)));
    // Half a step of the frame's 5-bit blue mantissa (rg11b10ufloat) up or down by the pixel and the
    // frame, so that the slow gradients of a dark sky show no bands.
    return vec4f(c * (1.0 + (dither(in.clip.xy) - 0.5) / 32.0), 1.0);
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
    // The moon hides the stars behind it.
    if (ahead <= 0.01 || show <= 0.0 || dot(dir, night.moon.xyz) > cos(night.pixel.w)) {
        return Star(vec4f(2.0, 2.0, 0.0, 1.0), corner, vec3f(0.0));
    }
    let right = look.right.xyz;
    let up = look.up.xyz;
    let at = vec2f(dot(dir, right) / dot(right, right), dot(dir, up) / dot(up, up)) / ahead + look.screen.xy;
    // The radius in render pixels is at least 1, so that a star covers a pixel's centre at every
    // jitter also below a render scale of 0.67; its light spreads over the radius that it takes.
    let pixels = max(STAR_RADIUS * (1.0 + 0.5 * clamp(1.0 - star.w / 2.0, 0.0, 1.5)) * look.screen.z, 1.0);
    let radius = pixels / look.screen.z;
    let light = STAR_LIGHT * pow(10.0, -STAR_STEP * star.w) * show / (radius * radius);
    return Star(vec4f(at + corner * pixels * night.pixel.xy, 0.0, 1.0), corner, tint.rgb * light);
}

@fragment
fn star_fragment(in: Star) -> @location(0) vec4f {
    // A Gaussian that falls to 0 at the quad's inscribed circle, so no square shows about a bright star.
    let fall = max(exp(-4.0 * dot(in.corner, in.corner)) - exp(-4.0), 0.0) / (1.0 - exp(-4.0));
    return vec4f(in.colour * fall, 1.0);
}
