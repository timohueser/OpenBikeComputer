// The mip levels of a shadow map (shadow.ts): each texel holds the least depth, the highest caster, of
// the four below it, so that a read of a coarse level tells whether any caster lies about a point.

@group(0) @binding(0) var finer: texture_depth_2d;

@vertex
fn mip_vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn mip_fragment(@builtin(position) at: vec4f) -> @builtin(frag_depth) f32 {
    let t = vec2u(at.xy) * 2u;
    return min(min(textureLoad(finer, t, 0), textureLoad(finer, t + vec2u(1u, 0u), 0)),
        min(textureLoad(finer, t + vec2u(0u, 1u), 0), textureLoad(finer, t + vec2u(1u, 1u), 0)));
}
