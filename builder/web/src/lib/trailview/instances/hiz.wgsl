// The depth pyramid of the last frame, for occlusion culling: level 0 holds the farthest depth of each
// block of pixels of the frame, each level above the farthest of 2 × 2 texels of the level below.
// The depth is reversed: the farthest is the least.

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var level0: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var below: texture_2d<f32>;
@group(0) @binding(3) var above: texture_storage_2d<r32float, write>;
// The last frame's pixels, x and y, and the pixels of a block a side.
@group(0) @binding(4) var<uniform> viewport: vec4u;

@compute @workgroup_size(8, 8)
fn first(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= textureDimensions(level0))) {
        return;
    }
    var least = 1.0;
    for (var j = 0u; j < viewport.z; j++) {
        for (var i = 0u; i < viewport.z; i++) {
            let at = min(id.xy * viewport.z + vec2u(i, j), viewport.xy - 1u);
            least = min(least, textureLoad(depth, at, 0));
        }
    }
    textureStore(level0, id.xy, vec4f(least));
}

@compute @workgroup_size(8, 8)
fn down(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= textureDimensions(above))) {
        return;
    }
    let last = textureDimensions(below) - 1u;
    let a = textureLoad(below, min(2u * id.xy, last), 0).x;
    let b = textureLoad(below, min(2u * id.xy + vec2u(1u, 0u), last), 0).x;
    let c = textureLoad(below, min(2u * id.xy + vec2u(0u, 1u), last), 0).x;
    let d = textureLoad(below, min(2u * id.xy + 1u, last), 0).x;
    textureStore(above, id.xy, vec4f(min(min(a, b), min(c, d))));
}
