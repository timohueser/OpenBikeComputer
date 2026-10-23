// What a producer of instances includes: `emit` puts one instance on the drawn surface into the draw of
// its model's tier, with a dithered fade between two tiers. A producer is a compute pass that finds its
// instances and their drawn heights; instances.ts sorts what the producers stage into one list per
// draw. Group 0 holds bindings 0 to 4; a producer's own bindings start at 8. Include it with
// `// #include ../instances/emit.wgsl`.
// #include ../terrain/drawn-height.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    // Metres per pixel at 1 m from the eye.
    pixel: f32,
}

// A model, as instances.ts writes it.
struct Model {
    // Per tier the distance in metres where it ends, 0 where the model has no such tier.
    until: vec4f,
    // Vertex units per model unit; a model is 1 tall.
    extent: f32,
    // The radius of a sphere about the middle of the model that holds it, in model units.
    radius: f32,
    // The share of a tier's end distance over which it fades into the next tier.
    fade: f32,
    // The farthest that a vertex lies from the model's axis, in model units: a crown's radius.
    spread: f32,
}

// One instance of a draw: its base in render space and its height in metres; its turn and width as
// the f16 pair (cos, sin) × width; its model; its look (TINT, FADE and APPEAR bytes, INCOMING flag); the draw.
struct Instance {
    at: vec4f,
    turn: u32,
    model: u32,
    look: u32,
    draw: u32,
}

struct Tally {
    staged: atomic<u32>,
    dropped: atomic<u32>,
    unused: vec2u,
    // Instances per draw, which sort.wgsl counts.
    draws: array<u32, 512>,
}

struct Params {
    // The ends of the tiers but the last are multiplied by `lod`: the render's pixels against those
    // that they are made for, times `detail`. The last ends where its model ends, times `detail`: below
    // 1 while the staged instances fill the pool.
    lod: f32,
    // Instances that a frame stages at most.
    capacity: u32,
    detail: f32,
    unused: u32,
}

// Where a tier of a model ends: a model's last tier ends where the model does, at any resolution.
fn tier_end(m: Model, tier: u32) -> f32 {
    let last = tier + 1u == TIERS || m.until[tier + 1u] <= 0.0;
    return m.until[tier] * select(params.lod, params.detail, last);
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> models: array<Model, 128>;
@group(0) @binding(2) var<storage, read_write> staged: array<Instance>;
@group(0) @binding(3) var<storage, read_write> tally: Tally;
@group(0) @binding(4) var<uniform> params: Params;

// Draws per model: three mesh tiers and the far tier, as TIERS of instances.ts.
const TIERS: u32 = 4u;
const INCOMING: u32 = 1u << 16u;

// The point of render space of a point of LOD space at height h above sea level: frame metres about
// the eye, dropped by the curvature.
fn render_at(s: vec2f, h: f32) -> vec3f {
    let xy = to_render(s);
    return vec3f(xy, h - frame.eye - frame.curvature * dot(xy, xy));
}

// Whether a sphere of render space meets the view, past the near plane.
fn in_view(centre: vec3f, radius: f32) -> bool {
    let m = frame.view_proj;
    let row = transpose(m);
    let w = row[3];
    for (var i = 0u; i < 4u; i++) {
        let plane = w + select(1.0, -1.0, (i & 1u) == 1u) * row[i >> 1u];
        if (dot(plane.xyz, centre) + plane.w < -radius * length(plane.xyz)) {
            return false;
        }
    }
    return dot(w.xyz, centre) + w.w > -radius * length(w.xyz);
}

fn stage(draw: u32, at: vec4f, turn: u32, model: u32, look: u32) {
    let k = atomicAdd(&tally.staged, 1u);
    if (k < params.capacity) {
        staged[k] = Instance(at, turn, model, look, draw);
    } else {
        atomicAdd(&tally.dropped, 1u);
    }
}

// Emits an instance of `model` standing at `base` (render space), `height` metres tall, turned by
// `yaw` and `width` times as wide as its model, its colour times `tint` (0.75 to 1.25), and drawn on
// the share `appear` of its pixels. In the last share `fade` of a tier it goes to the next by a dither.
fn emit(model: u32, base: vec3f, height: f32, yaw: f32, width: f32, tint: f32, appear: f32) {
    let m = models[model];
    if (appear <= 0.0 || !in_view(base + vec3f(0.0, 0.0, 0.5 * height), m.radius * height * max(width, 1.0))) {
        return;
    }
    let d = length(base);
    var tier = 0u;
    while (tier < TIERS && m.until[tier] > 0.0 && d > tier_end(m, tier)) {
        tier++;
    }
    if (tier == TIERS || m.until[tier] <= 0.0) {
        return;
    }
    let end = tier_end(m, tier);
    let x = clamp((d - end * (1.0 - m.fade)) / (end * m.fade), 0.0, 1.0);
    let at = vec4f(base, height);
    let turn = pack2x16float(width * vec2f(cos(yaw), sin(yaw)));
    let look = u32(round(clamp((tint - 0.75) * 2.0, 0.0, 1.0) * 255.0)) | (u32(round(clamp(appear, 0.0, 1.0) * 255.0)) << 24u);
    let fade = u32(round(x * 255.0)) << 8u;
    // Past its last tier an instance fades out.
    let next = tier + 1u < TIERS && m.until[tier + 1u] > 0.0;
    if (x < 1.0) {
        stage(model * TIERS + tier, at, turn, model, look | fade);
    }
    if (x > 0.0 && next) {
        stage(model * TIERS + tier + 1u, at, turn, model, look | fade | INCOMING);
    }
}
