// What a producer of instances includes: `emit` puts one instance on the drawn surface into the draw of
// its model's tier, with the hashes of a place (hash.wgsl). A producer is a compute pass that finds its instances and
// stands them on `stand_height` (drawn-mesh.wgsl); instances.ts sorts what the producers stage into one
// list per draw. Group 0 holds bindings 0 to 6; a
// producer's own bindings start at 8. Include it with `// #include ../instances/emit.wgsl`.
// #include ../ground/look.wgsl
// #include ../terrain/drawn-mesh.wgsl
// #include ../terrain/view.wgsl
// #include ../vegetation/phenology.wgsl
// #include hash.wgsl

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
    // How far the model takes its hue from the ground under it (draw.wgsl): 0 for a tree.
    tie: f32,
    // The height in metres of an instance whose tiers end at `until`; a taller one keeps each tier
    // farther, as it draws as large. 0: the tiers end at `until` at every height.
    sized: f32,
    // A tree's sink table (sinks of instances.ts), at the slopes SINK_STEP × (k + 1) for k 0 to 3 and
    // 4 to 7: how far a crown vertex lies below the line of that slope through the crown's base, at most.
    sink_low: vec4f,
    sink_high: vec4f,
    // A tree's crown: the luma of its mean colour as it shows lit, linear; 0 for a model that keeps its own colours (draw.wgsl).
    luma: f32,
}

// One instance of a draw: its base in render space and its height in metres; its turn and width as
// the f16 pair (cos, sin) × width; the ground's albedo under it, its square root as unorm8 × 3, and in
// the high byte its phase (2 bits) and the share of the sun at its base (6 bits); its look (tint, lift,
// snow and appear bytes); the draw, which names its model and tier.
struct Instance {
    at: vec4f,
    turn: u32,
    ground: u32,
    look: u32,
    draw: u32,
}

struct Tally {
    staged: atomic<u32>,
    dropped: atomic<u32>,
    unused: vec2u,
    // Instances per draw, which sort.wgsl counts.
    draws: array<u32, 1024>,
}

struct Params {
    // The ends of the tiers but the last are multiplied by `lod`: the render's pixels against those
    // that they are made for, times `detail`. The last ends where its model ends, times `detail`: below
    // 1 while the staged instances fill the pool.
    lod: f32,
    // Instances that a frame stages at most.
    capacity: u32,
    detail: f32,
    // The day of the year, 0 for 1 January: the phase of each tree (phenology.wgsl).
    day: f32,
    // The view, as VIEWS of instances.ts: 0 the eye's, SUN_VIEW the sun's, whose casters stage in their last tier.
    view: u32,
    // The sun's view: the least height in metres of an instance that casts (SunRun of instances.ts).
    least: f32,
    unused: vec2u,
}

const SUN_VIEW: u32 = 1u;

// Where a tier of an instance `height` metres tall ends: a model's last tier ends where the model does,
// at any resolution and height.
fn tier_end(m: Model, tier: u32, height: f32) -> f32 {
    let last = tier + 1u == TIERS || m.until[tier + 1u] <= 0.0;
    return m.until[tier] * select(params.lod * select(1.0, height / m.sized, m.sized > 0.0), params.detail, last);
}

@group(0) @binding(1) var<uniform> models: array<Model, 256>;
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
// Per model with phases (phases of instances.ts): x its model in each phase (8 bits each, winter
// first) for the mesh tiers, z for the impostor tier, and y its impostor tier + 1; y is 0 for a model
// that draws itself all year.
@group(0) @binding(7) var<uniform> phases: array<vec4u, 256>;

// Draws per model: three mesh tiers and the far tier, as TIERS of instances.ts.
const TIERS: u32 = 4u;
// The mesh tiers that draw without the dither (WHOLE_TIERS of instances.ts), and the slopes of the sink
// table (SINK_STEP of instances.ts).
const WHOLE_TIERS: u32 = 2u;
const SINK_STEP: f32 = 0.25;
// The terrain level whose height gives a tree its species and phase (105 m at 47° N), the same at every
// distance. Near the eye the pools hold finer terrain pages only: there a tree takes the drawn ground.
const BAND_LEVEL: u32 = 12u;

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

fn stage(draw: u32, at: vec4f, turn: u32, ground: u32, look: u32) {
    let k = atomicAdd(&tally.staged, 1u);
    if (k < params.capacity) {
        staged[k] = Instance(at, turn, ground, look, draw);
    } else {
        atomicAdd(&tally.dropped, 1u);
    }
}

fn byte_of(share: f32) -> u32 {
    return u32(round(clamp(share, 0.0, 1.0) * 255.0));
}

// Emits an instance of `model` standing at point `s` of LOD space on ground `z` metres above sea level,
// `height` metres tall, turned by `yaw` and `width` times as wide as its model, its colour times `tint` (0.75 to 1.25), and drawn on
// the share `appear` of its pixels. Its yaw and height pick where in a tier's fade it switches tiers.
fn emit(model: u32, s: vec2f, z: f32, height: f32, yaw: f32, width: f32, tint: f32, appear: f32) {
    emit_tree(model, s, z, height, yaw, width, tint, appear, 0.0, 0.0, pick_of(yaw, height), 0.0, vec3f(0.0), 0.0);
}

// A share 0 to 1 that is stable per instance, from its yaw and height.
fn pick_of(yaw: f32, height: f32) -> f32 {
    return f32(((bitcast<u32>(yaw) ^ bitcast<u32>(height)) * 2654435761u) >> 8u) / 16777216.0;
}

// How deep a tree sinks on a slope, in model units. Downhill the ground under a crown vertex drops by
// `slope` × its reach; the tree sinks so that this drop, less the sink, nowhere exceeds the vertex's
// height. A vertex below the crown's base counts as at the base, so no tree sinks on flat ground. A
// crown lifted to `lift` (lifted of draw.wgsl) squeezes its heights above its base by `raise`, which
// the table's slope undoes. `slope` is the rise per metre (stand_ground), at most 1.
fn slope_sink(m: Model, slope: f32, width: f32, lift: f32) -> f32 {
    if (m.crown < 0.0) {
        return 0.0;
    }
    let base = select(m.crown, lift, m.trunk != 0.0 && lift > m.crown);
    let raise = (1.0 - base) / (1.0 - m.crown);
    // The table between its slopes, from 0 at slope 0; past its last slope, along its last step.
    let t = min(slope, 1.0) * width / raise / SINK_STEP;
    let k = min(u32(t), 7u);
    let a = select(0.0, sink_at(m, max(k, 1u) - 1u), k > 0u);
    return max(0.0, raise * mix(a, sink_at(m, k), t - f32(k)) - base);
}

// Entry k of a model's sink table.
fn sink_at(m: Model, k: u32) -> f32 {
    return select(m.sink_high, m.sink_low, k < 4u)[k % 4u];
}

// As `emit`, with the crown's base raised to `lift` of the height, on ground of the rise `slope` per
// metre, which sinks it (slope_sink), the share `snow` of snow on its upward faces, and the albedo of
// the `ground` under it, which a model with a tie and a forest tree take (draw.wgsl). The instance switches to the next tier at
// the share `pick` (0 to 1, stable per instance) of a tier's last share `fade`, and past its last tier
// ends there: a still view draws each instance whole, in one tier. A `species` with phases draws the model
// of the tree's phase on the date, by its pick and its `elevation` in metres, which a producer keeps
// the same at every distance (BAND_LEVEL).
fn emit_tree(species: u32, s: vec2f, z: f32, height: f32, yaw: f32, width: f32, tint: f32, appear: f32, lift: f32, slope: f32, pick: f32, snow: f32, ground: vec3f, elevation: f32) {
    if (appear <= 0.0) {
        return;
    }
    let base = render_at(s, z);
    var model = species;
    var phase = PHASE_SUMMER;
    let year = phases[species];
    if (year.y != 0u) {
        phase = phase_of(unit_float(mix_bits(bitcast<u32>(pick) ^ 0x68e31da4u)), params.day, elevation);
        model = (year.x >> (8u * phase)) & 255u;
    }
    var m = models[model];
    let d = length(base);
    var tier = 0u;
    if (params.view == SUN_VIEW) {
        if (height < params.least) {
            return;
        }
        // The sun's draw of an impostor tier draws the coarsest mesh.
        while (tier + 1u < TIERS && m.until[tier + 1u] > 0.0) {
            tier++;
        }
    } else {
        while (tier < TIERS && m.until[tier] > 0.0 && d > tier_end(m, tier, height) * (1.0 - m.fade * pick)) {
            tier++;
        }
        // An instance drawn on a share of its pixels draws with the dither.
        if (appear < 1.0) {
            tier = max(tier, WHOLE_TIERS);
        }
    }
    if (tier == TIERS || m.until[tier] <= 0.0) {
        return;
    }
    if (tier + 1u == year.y) {
        model = (year.z >> (8u * phase)) & 255u;
        m = models[model];
    }
    let at = base - vec3f(0.0, 0.0, height * slope_sink(m, slope, width, lift));
    let centre = at + vec3f(0.0, 0.0, 0.5 * height);
    let radius = m.radius * height * max(width, 1.0);
    if (!visible(centre, radius) || hidden(centre, radius)) {
        return;
    }
    let look = byte_of((tint - 0.75) * 2.0) | (byte_of(lift) << 8u) | (byte_of(snow) << 16u) | (byte_of(appear) << 24u);
    let sun = u32(round(sun_at(s) * 63.0));
    stage(model * TIERS + tier, vec4f(at, height), pack2x16float(width * vec2f(cos(yaw), sin(yaw))), pack4x8unorm(vec4f(sqrt(ground), 0.0)) | ((phase | (sun << 2u)) << 24u), look);
}
