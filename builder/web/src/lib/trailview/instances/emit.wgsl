// What a producer of instances includes: `emit` puts one instance on the drawn surface into the draw of
// its model's tier. A producer is a compute pass that finds its instances and stands them on `stand_height`;
// instances.ts sorts what the producers stage into one list per draw. Group 0 holds bindings 0 to 6; a
// producer's own bindings start at 8. Include it with `// #include ../instances/emit.wgsl`.
// #include ../terrain/drawn-mesh.wgsl
// #include ../terrain/view.wgsl

// A model, as instances.ts writes it.
struct Model {
    // Per tier the distance in metres where it ends, 0 where the model has no such tier.
    until: vec4f,
    // Vertex units per model unit; a model is 1 tall.
    extent: f32,
    // The radius of a sphere about the middle of the model that holds it, in model units.
    radius: f32,
    // The share of a tier's end distance over which its instances switch to the next tier.
    fade: f32,
    // The farthest that a vertex lies from the model's axis, in model units: a crown's radius.
    spread: f32,
    // A tree's crown: the height of its lowest point in model units; negative for a model that is no tree.
    crown: f32,
    // 1 for a model whose meshes mark their trunk (Mesh.trunk of instances.ts), else 0.
    trunk: f32,
    unused2: vec2f,
}

// One instance of a draw: its base in render space and its height in metres; its turn and width as
// the f16 pair (cos, sin) × width; its model; its look (tint, lift, snow and appear bytes); the draw.
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

@group(0) @binding(1) var<uniform> models: array<Model, 128>;
@group(0) @binding(2) var<storage, read_write> staged: array<Instance>;
@group(0) @binding(3) var<storage, read_write> tally: Tally;
@group(0) @binding(4) var<uniform> params: Params;

// The last frame, whose depth pyramid (hiz.wgsl) culls what it hid.
struct Occlusion {
    // Its view projection.
    last: mat4x4f,
    // xyz: the eye's move since then. w: 1 when the pyramid holds the last frame, else 0.
    moved: vec4f,
    // xy: its render pixels. z: its pixels per texel of level 0 a side.
    pixels: vec4f,
}
@group(0) @binding(5) var<uniform> occlusion: Occlusion;
@group(0) @binding(6) var pyramid: texture_2d<f32>;

// Draws per model: three mesh tiers and the far tier, as TIERS of instances.ts.
const TIERS: u32 = 4u;
// Instances nearer than this stand on the drawn height; farther ones on the bilinear heights of the two
// levels that the mesh blends there, which meet the mesh within a pixel.
const MESH_HEIGHT_M: f32 = 400.0;

// The ground under a point of LOD space, `u` in whole world units: where the mesh draws it.
fn stand_height(s: vec2f, u: vec2i) -> f32 {
    if (length(s) < MESH_HEIGHT_M) {
        return plain_height(s);
    }
    // As grid_vertex: a node of level `fine` goes to the coarser level's height by its morph.
    let q = lod(s);
    let fine = u32(ceil(q));
    let h = units_height(u, fine);
    let morph = f32(fine) - q;
    if (morph <= 0.0) {
        return h;
    }
    let coarse = units_height(u, fine - 1u);
    return select(select(h, coarse, known(coarse)), mix(h, coarse, morph), known(h) && known(coarse));
}

// The NDC span of a sphere in front of the eye along the clip axis `row`, `w` the clip w row: the two
// planes through the eye that touch it, a − k·b = ±r·|A − k·B| for the rows' xyz A, B and values a, b.
fn ndc_span(row: vec4f, w: vec4f, centre: vec3f, radius: f32) -> vec2f {
    let a = dot(row.xyz, centre) + row.w;
    let b = dot(w.xyz, centre) + w.w;
    let r2 = radius * radius;
    let qa = b * b - r2 * dot(w.xyz, w.xyz);
    let qb = a * b - r2 * dot(row.xyz, w.xyz);
    let qc = a * a - r2 * dot(row.xyz, row.xyz);
    let root = sqrt(max(qb * qb - qa * qc, 0.0));
    return vec2f(qb - root, qb + root) / qa;
}

// Whether the last frame hid a sphere of render space: its nearest point lay behind the farthest depth
// of the pixels that it covered. A sphere at the frame's edge or too near the eye is never hidden.
fn hidden(centre: vec3f, radius: f32) -> bool {
    if (occlusion.moved.w == 0.0) {
        return false;
    }
    let p = centre + occlusion.moved.xyz;
    let row = transpose(occlusion.last);
    let clip = occlusion.last * vec4f(p, 1.0);
    let w = clip.w - radius * length(row[3].xyz);
    if (w <= 1.0) {
        return false;
    }
    let x = ndc_span(row[0], row[3], p, radius);
    let y = ndc_span(row[1], row[3], p, radius);
    // Render pixels are y down.
    let lo = (vec2f(x.x, -y.y) * 0.5 + 0.5) * occlusion.pixels.xy;
    let hi = (vec2f(x.y, -y.x) * 0.5 + 0.5) * occlusion.pixels.xy;
    if (any(lo < vec2f(0.0)) || any(hi >= occlusion.pixels.xy)) {
        return false;
    }
    let a = lo / occlusion.pixels.z;
    let b = hi / occlusion.pixels.z;
    // The level where the sphere spans two texels at most each way.
    let level = u32(ceil(log2(max(max(b.x - a.x, b.y - a.y), 1.0))));
    if (level >= textureNumLevels(pyramid)) {
        return false;
    }
    let ta = vec2u(a) >> vec2u(level);
    let tb = vec2u(b) >> vec2u(level);
    let far = min(min(textureLoad(pyramid, ta, level).x, textureLoad(pyramid, vec2u(tb.x, ta.y), level).x),
        min(textureLoad(pyramid, vec2u(ta.x, tb.y), level).x, textureLoad(pyramid, tb, level).x));
    // The depth is reversed: NEAR / w.
    return clip.z / w < far;
}

fn stage(draw: u32, at: vec4f, turn: u32, model: u32, look: u32) {
    let k = atomicAdd(&tally.staged, 1u);
    if (k < params.capacity) {
        staged[k] = Instance(at, turn, model, look, draw);
    } else {
        atomicAdd(&tally.dropped, 1u);
    }
}

fn byte_of(share: f32) -> u32 {
    return u32(round(clamp(share, 0.0, 1.0) * 255.0));
}

// Emits an instance of `model` standing at `base` (render space), `height` metres tall, turned by
// `yaw` and `width` times as wide as its model, its colour times `tint` (0.75 to 1.25), and drawn on
// the share `appear` of its pixels. Its yaw and height pick where in a tier's fade it switches tiers.
fn emit(model: u32, base: vec3f, height: f32, yaw: f32, width: f32, tint: f32, appear: f32) {
    emit_tree(model, base, height, yaw, width, tint, appear, 0.0, pick_of(yaw, height), 0.0);
}

// A share 0 to 1 that is stable per instance, from its yaw and height.
fn pick_of(yaw: f32, height: f32) -> f32 {
    return f32(((bitcast<u32>(yaw) ^ bitcast<u32>(height)) * 2654435761u) >> 8u) / 16777216.0;
}

// As `emit`, with the crown's base raised to `lift` of the height and the share `snow` of snow on its
// upward faces (draw.wgsl). The instance switches to the next tier at the share `pick` (0 to 1, stable
// per instance) of a tier's last share `fade`, and past its last tier ends there: a still view draws
// each instance whole, in one tier.
fn emit_tree(model: u32, base: vec3f, height: f32, yaw: f32, width: f32, tint: f32, appear: f32, lift: f32, pick: f32, snow: f32) {
    let m = models[model];
    let centre = base + vec3f(0.0, 0.0, 0.5 * height);
    let radius = m.radius * height * max(width, 1.0);
    if (appear <= 0.0 || !visible(centre, radius) || hidden(centre, radius)) {
        return;
    }
    let d = length(base);
    var tier = 0u;
    while (tier < TIERS && m.until[tier] > 0.0 && d > tier_end(m, tier) * (1.0 - m.fade * pick)) {
        tier++;
    }
    if (tier == TIERS || m.until[tier] <= 0.0) {
        return;
    }
    let look = byte_of((tint - 0.75) * 2.0) | (byte_of(lift) << 8u) | (byte_of(snow) << 16u) | (byte_of(appear) << 24u);
    stage(model * TIERS + tier, vec4f(base, height), pack2x16float(width * vec2f(cos(yaw), sin(yaw))), model, look);
}
