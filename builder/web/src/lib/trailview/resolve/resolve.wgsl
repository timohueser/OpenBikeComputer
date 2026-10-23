// The resolve: temporal anti-aliasing of the jittered frames at the render scale, a light sharpening
// and `finish()`, in one pass at the output size. It writes the canvas and the history. The world
// stands still, so the depth and the camera of the last frame reproject a pixel. Colours mix
// tone-mapped (Karis), so a bright glint weighs no more than a dim pixel.
// #include ../light/shade.wgsl

struct Resolve {
    // The last frame's view projection, without its jitter.
    last_view_proj: mat4x4f,
    // xyz: the eye minus the last frame's eye, in metres. w: metres of drop per square metre of distance.
    moved: vec4f,
    // xy: this frame's render size in pixels. zw: unused.
    render: vec4f,
    // xy: this frame's jitter in render pixels, y down. z: the near plane in metres. w: the sharpening.
    jitter: vec4f,
    // x: the weight of this frame against the history; 1 drops the history. y: 1 clips the history to
    // this frame's neighbourhood and tests its depth. z: 1 copies the frame (the debug views).
    blend: vec4f,
}

@group(0) @binding(0) var<uniform> resolve: Resolve;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
// The accumulated colour, compressed and encoded (`encode`).
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var bilinear: sampler;
@group(0) @binding(5) var nearest_texel: sampler;
// The nearest depth about each pixel of the last frame, times DEPTH_SCALE.
@group(0) @binding(6) var last_depth: texture_2d<f32>;

struct Out {
    @location(0) canvas: vec4f,
    @location(1) history: vec4f,
    @location(2) depth: vec4f,
}

// Reversed Z: when every depth of the last frame about a point is larger (nearer) than this share of
// the point's expected depth, the point was hidden then.
const HIDDEN: f32 = 1.1;
// The neighbourhood that the history keeps to while the camera moves, in standard deviations.
const SPREAD: f32 = 1.25;
// Keeps a far depth (reversed Z: near / distance) in the normal range of a half float.
const DEPTH_SCALE: f32 = 1e4;
const CROSS = array<vec2i, 5>(vec2i(0, 0), vec2i(1, 0), vec2i(-1, 0), vec2i(0, 1), vec2i(0, -1));

@vertex
fn resolve_vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i & 1u) * 4.0 - 1.0, f32(i >> 1u) * 4.0 - 1.0, 0.0, 1.0);
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

fn compress(c: vec3f) -> vec3f {
    return c / (1.0 + luminance(c));
}

fn expand(c: vec3f) -> vec3f {
    return c / max(1.0 - luminance(c), 1e-4);
}

// A compressed colour for the 10 bits of the history: halved, so channels up to 2 fit, and square-rooted
// for the steps of dark tones. The Catmull-Rom filters the history in this form.
fn encode(c: vec3f) -> vec3f {
    return sqrt(0.5 * c);
}

fn decode(e: vec3f) -> vec3f {
    return 2.0 * e * e;
}

fn ycocg(c: vec3f) -> vec3f {
    return vec3f(dot(c, vec3f(0.25, 0.5, 0.25)), dot(c, vec3f(0.5, 0.0, -0.5)), dot(c, vec3f(-0.25, 0.5, -0.25)));
}

fn rgb(c: vec3f) -> vec3f {
    return vec3f(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// The history at a point, tone-mapped: Catmull-Rom in five bilinear reads, so a moving view stays sharp.
fn history_at(uv: vec2f) -> vec3f {
    let size = vec2f(textureDimensions(history));
    let p = uv * size;
    let t1 = floor(p - 0.5) + 0.5;
    let f = p - t1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let t0 = (t1 - 1.0) / size;
    let t3 = (t1 + 2.0) / size;
    let t12 = (t1 + w2 / w12) / size;
    let c = textureSampleLevel(history, bilinear, vec2f(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y)
        + textureSampleLevel(history, bilinear, vec2f(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y)
        + textureSampleLevel(history, bilinear, t12, 0.0).rgb * (w12.x * w12.y)
        + textureSampleLevel(history, bilinear, vec2f(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y)
        + textureSampleLevel(history, bilinear, vec2f(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    let total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return decode(max(c / total, vec3f(0.0)));
}

// Moves a colour toward the centre of a box until it is inside it.
fn clip_box(c: vec3f, centre: vec3f, extent: vec3f) -> vec3f {
    let d = c - centre;
    let units = abs(d) / max(extent, vec3f(1e-5));
    let m = max(units.x, max(units.y, units.z));
    return select(c, centre + d / m, m > 1.0);
}

// A bilinear read of the history for the sharpening, clipped to the box when `clip` is true.
fn neighbour(uv: vec2f, clip: bool, centre: vec3f, extent: vec3f) -> vec3f {
    let c = decode(textureSampleLevel(history, bilinear, uv, 0.0).rgb);
    if (clip) {
        return rgb(clip_box(ycocg(c), centre, extent));
    }
    return c;
}

@fragment
fn resolve_fragment(@builtin(position) at: vec4f) -> Out {
    let size = resolve.render.xy;
    let last = vec2i(size) - 1;
    if (resolve.blend.z > 0.5) {
        let c = textureLoad(scene, min(vec2i(at.xy), last), 0).rgb;
        return Out(vec4f(c, 1.0), vec4f(encode(compress(c)), 1.0), vec4f(0.0));
    }
    let output = vec2f(textureDimensions(history));
    // Render pixel i holds the point at i + 0.5 - jitter: the pixel about this pixel's point and its
    // four neighbours, weighed by their distance (a Gaussian a quarter of a pixel wide), give this
    // frame's colour and its neighbourhood.
    let c = at.xy * size / output + resolve.jitter.xy;
    let base = vec2i(floor(c));
    var sum = vec3f(0.0);
    var total = 0.0;
    var m1 = vec3f(0.0);
    var m2 = vec3f(0.0);
    var nearest = 0.0;
    for (var j = 0; j < 5; j++) {
        let o = base + CROSS[j];
        let p = clamp(o, vec2i(0), last);
        let s = compress(textureLoad(scene, p, 0).rgb);
        let d = vec2f(o) + 0.5 - c;
        let w = exp(-9.16 * dot(d, d));
        sum += w * s;
        total += w;
        let y = ycocg(s);
        m1 += y;
        m2 += y * y;
        nearest = max(nearest, textureLoad(depth, p, 0));
    }
    let current = sum / total;
    let mean = m1 / 5.0;
    let extent = SPREAD * sqrt(max(m2 / 5.0 - mean * mean, vec3f(0.0)));
    let clip = resolve.blend.y > 0.5;

    // The pixel's point on its view ray at the nearest depth about it (reversed Z: depth = near /
    // distance along the view), in the last frame. The curvature drop is relative to the eye.
    let ndc = vec2f(at.x / output.x * 2.0 - 1.0, 1.0 - at.y / output.y * 2.0);
    let ray = look.forward.xyz + ndc.x * look.right.xyz + ndc.y * look.up.xyz;
    var was = resolve.last_view_proj * vec4f(ray, 0.0);
    if (nearest > 0.0) {
        let p = ray * (resolve.jitter.z / nearest);
        let xy = p.xy + resolve.moved.xy;
        let z = p.z + resolve.moved.z + resolve.moved.w * (dot(p.xy, p.xy) - dot(xy, xy));
        was = resolve.last_view_proj * vec4f(xy, z, 1.0);
    }
    let n = was.xy / max(was.w, 1e-9);
    let uv = vec2f(0.5 + 0.5 * n.x, 0.5 - 0.5 * n.y);
    var weight = resolve.blend.x;
    if (was.w <= 0.0 || any(uv < vec2f(0.0)) || any(uv > vec2f(1.0))) {
        weight = 1.0;
    }
    var h = vec3f(0.0);
    if (weight < 1.0) {
        h = history_at(uv);
        if (clip) {
            // Hidden in the last frame: every depth about the point there is nearer than the point.
            let expected = select(0.0, resolve.jitter.z / was.w, nearest > 0.0);
            let around = textureGather(0, last_depth, nearest_texel, uv) / DEPTH_SCALE;
            if (min(min(around.x, around.y), min(around.z, around.w)) > expected * HIDDEN) {
                weight = 1.0;
            }
            h = rgb(clip_box(ycocg(h), mean, extent));
        }
    }
    let blended = mix(h, current, weight);
    // The canvas sharpens the blend against the history about the point, the image of a frame
    // before, kept within their range so that no edge rings. While the camera moves, those reads keep
    // to the clip box of the history, so no stale colour widens the range.
    var shown = blended;
    if (weight < 1.0 && resolve.jitter.w > 0.0) {
        let texel = 1.0 / output;
        let a = neighbour(uv + vec2f(texel.x, 0.0), clip, mean, extent);
        let b = neighbour(uv - vec2f(texel.x, 0.0), clip, mean, extent);
        let e = neighbour(uv + vec2f(0.0, texel.y), clip, mean, extent);
        let f = neighbour(uv - vec2f(0.0, texel.y), clip, mean, extent);
        let low = min(blended, min(min(a, b), min(e, f)));
        let high = max(blended, max(max(a, b), max(e, f)));
        shown = clamp(blended + resolve.jitter.w * (blended - 0.25 * (a + b + e + f)), low, high);
    }
    return Out(vec4f(finish(expand(shown)), 1.0), vec4f(encode(blended), 1.0), vec4f(nearest * DEPTH_SCALE));
}
