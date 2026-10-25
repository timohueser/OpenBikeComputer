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
