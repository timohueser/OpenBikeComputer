// A mip level of an atlas layer from the level below: the mean of 2 × 2 texels. The bake writes colour
// and normal times the cover, with 0 where nothing covers, so the means stay true at the crown's edge.

@group(0) @binding(0) var below_albedo: texture_2d_array<f32>;
@group(0) @binding(1) var below_normal: texture_2d_array<f32>;
@group(0) @binding(2) var above_albedo: texture_storage_2d_array<rgba8unorm, write>;
@group(0) @binding(3) var above_normal: texture_storage_2d_array<rgba8unorm, write>;
// As Bake of impostor-bake.wgsl.
struct Bake {
    model: u32,
    radius: f32,
    layer: u32,
    unused: u32,
}
@group(1) @binding(0) var<uniform> bake: Bake;

fn mean(below: texture_2d_array<f32>, at: vec2u) -> vec4f {
    return 0.25 * (textureLoad(below, at, bake.layer, 0) + textureLoad(below, at + vec2u(1u, 0u), bake.layer, 0)
        + textureLoad(below, at + vec2u(0u, 1u), bake.layer, 0) + textureLoad(below, at + 1u, bake.layer, 0));
}

@compute @workgroup_size(8, 8)
fn down(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= textureDimensions(above_albedo))) {
        return;
    }
    textureStore(above_albedo, id.xy, bake.layer, mean(below_albedo, 2u * id.xy));
    textureStore(above_normal, id.xy, bake.layer, mean(below_normal, 2u * id.xy));
}

// Per layer, mip level and frame, the alpha above which a texel draws (`covers` of impostor.wgsl): the
// texels above it cover the share of the frame that the level's mean alpha holds, which is the mesh's.
@group(2) @binding(0) var atlas_albedo: texture_2d_array<f32>;
@group(2) @binding(1) var<storage, read_write> covers: array<f32>;

var<workgroup> alphas: array<f32, 256>;
var<workgroup> threshold: atomic<u32>;

// One workgroup per frame (x) and level above the first (y) of the 64 frames of 32 texels and 6 levels
// (FRAMES and ATLAS_LEVELS of instances.ts): a frame there has at most 16 × 16 texels.
@compute @workgroup_size(256)
fn cover(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) i: u32) {
    let level = group.y + 1u;
    let side = 32u >> level;
    let n = side * side;
    let at = (bake.layer * 6u + level) * 64u + group.x;
    if (i < n) {
        alphas[i] = textureLoad(atlas_albedo, vec2u(group.x % 8u, group.x / 8u) * side + vec2u(i % side, i / side), bake.layer, level).a;
    }
    if (i == 0u) {
        atomicStore(&threshold, 0u);
        // The bake draws level 0 whole or not at all.
        if (level == 1u) {
            covers[at - 64u] = 0.5;
        }
    }
    workgroupBarrier();
    // The cover in texels, and the texels above this one's alpha. The greatest alpha with at least that
    // many above it is the threshold; floats of one sign order as their bits.
    var sum = 0.0;
    var above = 0u;
    let a = alphas[min(i, n - 1u)];
    for (var j = 0u; j < n; j++) {
        sum += alphas[j];
        above += u32(alphas[j] > a);
    }
    if (i < n && above >= u32(floor(sum + 0.5))) {
        atomicMax(&threshold, bitcast<u32>(a));
    }
    workgroupBarrier();
    if (i == 0u) {
        covers[at] = bitcast<f32>(atomicLoad(&threshold));
    }
}
