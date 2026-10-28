// Light and air, the same for every surface, in linear light: the sky colour, the three toon bands of
// sunlight, the haze and the valley mist, and `finish()` to the screen, which only the resolve calls.
// A port of SKY, FINISH and SHADE of the prototype's src/shaders.ts, without the cloud layer. Include
// it with `// #include ../light/shade.wgsl`.

struct Look {
    // The light that shades and shadows the scene: the sun, or the moon once the sun's light is gone.
    // xyz: toward it, in render space. w: its bearing in turns clockwise from north, as LOD space has it.
    sun: vec4f,
    // rgb: its light. w: how high the horizon's colour reaches in the sky (light.ts `rise`).
    sun_colour: vec4f,
    // rgb: the sky light on a face that points up. w: night, 0 by day to 1.
    ambient: vec4f,
    // rgb: the sky at the zenith. w: the width of the steps between the light bands.
    zenith: vec4f,
    // rgb: the sky at the horizon. w: the share of smooth light that the bands keep.
    horizon: vec4f,
    // The grade: rgb the tint of shadows, w the saturation.
    shadow: vec4f,
    // rgb: the tint of light. w: wet ground, 0 to 1.
    light: vec4f,
    // x: the visibility at sea level in metres. y: the scale height of the haze. z: the top of the
    // valley mist above sea level. w: the eye's height above sea level.
    haze: vec4f,
    // xy: the eye in the frame, metres east and north. z, w: unused.
    place: vec4f,
    // The bands of variegated rock: red, gypsum white, green-grey.
    variegated: array<vec4f, 3>,
    // The sky pass: the view ray of a point of clip space is forward + x right + y up.
    right: vec4f,
    up: vec4f,
    forward: vec4f,
    // x: the finest ground level. y, z: the materials and the lithologies of the vocabulary. w: unused.
    codes: vec4u,
    // x: the debug view of terrain.wgsl. y: the jittered frame's sample, which turns the dithers. z, w: unused.
    modes: vec4u,
    // xy: this frame's jitter in clip space. z: render pixels per output pixel, the render scale. w: unused.
    screen: vec4f,
    // xyz: toward the sun, which the sky's glow follows also while the moon is the light. w: the warm
    // glow about a low sun.
    sky_sun: vec4f,
    // rgb: the deep colour of the sea of the regions about the eye, so that all the sea in view has one
    // colour. w: unused.
    sea: vec4f,
}

@group(2) @binding(0) var<uniform> look: Look;

const GLOW = vec3f(1.0, 0.62, 0.36);

// A dither value of a pixel in [0, 1): interleaved gradient noise, turned by the golden ratio with
// each jittered frame, so the frames of a still view take evenly spread values and their mean is smooth.
fn dither(pixel: vec2f) -> f32 {
    let ign = fract(52.9829189 * fract(dot(floor(pixel), vec2f(0.06711056, 0.00583715))));
    return fract(ign + 0.6180340 * f32(look.modes.y));
}

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.3, 0.59, 0.11));
}

fn sky(dir: vec3f) -> vec3f {
    let up = clamp(dir.z, 0.0, 1.0);
    let c = mix(look.horizon.rgb, look.zenith.rgb, pow(max(up, 1e-6), look.sun_colour.w));
    let toward = max(dot(normalize(dir.xy + 1e-6), normalize(look.sky_sun.xy + 1e-6)), 0.0);
    let near = max(dot(dir, look.sky_sun.xyz), 0.0);
    return c + GLOW * look.sky_sun.w * (0.35 * pow(toward, 3.0) * exp(-5.0 * up) + 0.5 * pow(near, 24.0));
}

// Linear light to the screen: a hue-keeping tone curve (Khronos PBR Neutral), then a grade that leans
// shadows cool and light warm, then sRGB.
fn finish(colour: vec3f) -> vec3f {
    var c = colour;
    let low = min(c.r, min(c.g, c.b));
    c -= select(vec3f(0.04), vec3f(low - 6.25 * low * low), low < 0.08);
    let peak = max(c.r, max(c.g, c.b));
    if (peak > 0.76) {
        let top = 1.0 - 0.0576 / (peak - 0.52);
        c *= top / peak;
        c = mix(c, vec3f(top), 1.0 - 1.0 / (0.15 * (peak - top) + 1.0));
    }
    let l = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    c += (1.0 - smoothstep(0.0, 0.25, l)) * look.shadow.rgb + smoothstep(0.4, 0.9, l) * look.light.rgb;
    c = max(mix(vec3f(l), c, look.shadow.w), vec3f(0.0));
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, 12.92 * c, c < vec3f(0.0031308));
}

// Three soft bands of sunlight, with the share `relief` of the smooth term kept, so a high sun still
// shows relief. A face out of the sun keeps the cool sky light, never black; `open` is how much of the
// sky a hollow still sees. Steep faces also catch light thrown up from the ground below.
fn light_up(albedo: vec3f, n: vec3f, shadow: f32, open: f32, relief: f32) -> vec3f {
    let lit = max(dot(n, look.sun.xyz), 0.0) * shadow;
    let softness = look.zenith.w;
    let band = 0.45 * smoothstep(0.03, 0.03 + softness, lit) + 0.55 * smoothstep(0.32, 0.32 + softness, lit);
    return albedo * (look.ambient.rgb * ((0.85 + 0.15 * n.z) * open + 0.2 * (1.0 - abs(n.z))) + look.sun_colour.rgb * mix(band, lit, relief));
}

// The haze between the eye and a point `view` metres from it, at `altitude` above sea level.
fn atmosphere(colour: vec3f, view: vec3f, altitude: f32) -> vec3f {
    let d = length(view);
    // Haze lit from behind glows in the light's colour, most of all when the sun is low.
    let toward = max(dot(view / max(d, 1e-3), look.sun.xyz), 0.0);
    let haze = sky(normalize(vec3f(view.xy, 0.0) + vec3f(0.0, 0.0, 1e-6)))
        + look.sun_colour.rgb * (0.12 + look.sky_sun.w) * (0.25 * pow(toward, 6.0) + 0.6 * pow(toward, 40.0));
    // Height fog: the mean of exp(-altitude / H) along the sight line.
    let a0 = look.haze.w;
    let h = look.haze.y;
    let mean = select(h * (exp(-a0 / h) - exp(-altitude / h)) / (altitude - a0), exp(-a0 / h), abs(altitude - a0) < 1.0);
    var c = mix(colour, haze, 1.0 - exp(-d * mean / look.haze.x));
    // Valley mist: a pale layer below its top, thicker with distance.
    let mist = clamp((look.haze.z - altitude) / 150.0, 0.0, 1.0) * (1.0 - exp(-d / 1500.0));
    c = mix(c, mix(haze, vec3f(dot(haze, vec3f(0.333)) * 1.15), 0.5), mist);
    // At night the eye loses colour and sees blue.
    return mix(c, dot(c, vec3f(0.3, 0.55, 0.15)) * vec3f(0.7, 0.9, 1.5), 0.8 * look.ambient.w);
}
