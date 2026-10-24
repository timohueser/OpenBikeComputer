// The frame of the terrain in group 0, binding 0, for the passes that put things on the drawn surface:
// render space and the view's frustum. Include it with `// #include ../terrain/view.wgsl`.
// #include drawn-height.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    // Render space to clip space; reversed Z, infinite far plane.
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    // Metres per pixel at 1 m from the eye.
    pixel: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;

// The point of render space of a point of LOD space at z metres above sea level: frame metres about
// the eye, dropped by the curvature.
fn render_at(s: vec2f, z: f32) -> vec3f {
    var p = vec3f(to_render(s), z - frame.eye);
    p.z -= frame.curvature * dot(p.xy, p.xy);
    return p;
}

// Whether a sphere of render space meets the view's left, right, bottom and top planes.
fn visible(p: vec3f, radius: f32) -> bool {
    let row = transpose(frame.view_proj);
    for (var i = 0u; i < 4u; i++) {
        let plane = row[3] + select(1.0, -1.0, (i & 1u) == 1u) * row[i >> 1u];
        if (dot(plane.xyz, p) + plane.w < -radius * length(plane.xyz)) {
            return false;
        }
    }
    return true;
}
