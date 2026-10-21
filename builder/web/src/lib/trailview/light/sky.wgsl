// The sky behind the terrain: the sky colour, a halo about the sun and a disk about twice the real one.
// One triangle covers the screen at depth 0, where the reversed-Z depth test leaves only the pixels
// that no terrain covers. A port of the prototype's skyFragment without the clouds.
// #include shade.wgsl

struct Sky {
    @builtin(position) clip: vec4f,
    @location(0) ndc: vec2f,
}

@vertex
fn sky_vertex(@builtin(vertex_index) i: u32) -> Sky {
    let ndc = vec2f(f32(i & 1u) * 4.0 - 1.0, f32(i >> 1u) * 4.0 - 1.0);
    return Sky(vec4f(ndc, 0.0, 1.0), ndc);
}

@fragment
fn sky_fragment(in: Sky) -> @location(0) vec4f {
    let dir = normalize(look.forward.xyz + in.ndc.x * look.right.xyz + in.ndc.y * look.up.xyz);
    let near = max(dot(dir, look.sun.xyz), 0.0);
    var c = sky(dir) + look.sun_colour.rgb * (0.06 * pow(near, 16.0) + 0.35 * pow(near, 900.0));
    let disk = smoothstep(0.0095, 0.0075, acos(clamp(dot(dir, look.sun.xyz), -1.0, 1.0)));
    c = mix(c, look.sun_colour.rgb * 3.0 + 0.3, disk * step(0.001, dot(look.sun_colour.rgb, vec3f(1.0))));
    return vec4f(finish(c), 1.0);
}
