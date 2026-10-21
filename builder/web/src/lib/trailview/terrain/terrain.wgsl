// #include unknown.wgsl
// The terrain mesh: one grid of 64 × 64 cells, instanced per node. A vertex morphs toward the next
// coarser grid by its node's level − LOD, as `grid_vertex` gives it, and drops with the curvature.
// #include drawn-height.wgsl
// #include ../ground/look.wgsl

struct Frame {
    // Camera-relative world (x east, y north, z up) to clip space; reversed Z, infinite far plane.
    view_proj: mat4x4f,
    // xyz: the direction to the sun. w: the sunlight.
    sun: vec4f,
    // The eye's height above sea level.
    eye: f32,
    // Metres of drop per square metre of distance: Earth curvature with refraction.
    curvature: f32,
    // The index of the debug view: 0 off, 1 normals, 2 LOD, 3 cracks, 4 surface; look.wgsl draws the others.
    debug: u32,
}

struct Node {
    // The node's north-west corner in whole units about the anchor.
    origin: vec2i,
    level: u32,
    unused: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;

const CELLS = 64u;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32) -> Varyings {
    let node = nodes[instance];
    let grid = vec2i(vec2u(index % (CELLS + 1u), index / (CELLS + 1u)));
    let v = grid_vertex(node.origin + grid * (1 << (24u - node.level)), node.level);
    // A vertex of unknown height stands on the ground under the eye; `mark_known` drops each triangle that has one.
    var p = vec3f(to_render(v.s), select(-surface.above, v.h - frame.eye, known(v.h)));
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: Varyings;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.s = v.s;
    out.relative = p;
    mark_known(&out, known(v.h));
    out.h = v.h;
    out.level = node.level;
    return out;
}

fn hue(level: f32) -> vec3f {
    let h = fract(level * 0.29 + vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0));
    return clamp(abs(h * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

@fragment
fn fragment_main(in: Fragment) -> @location(0) vec4f {
    drop_unknown(in);
    let m = drawn_normal(in.s);
    // LOD space axes turn into render space axes with the linear part of the cubic.
    let n = normalize(vec3f(m.x * vec2f(coefficient(1u), coefficient(11u)) + m.y * vec2f(coefficient(2u), coefficient(12u)), m.z));
    let light = max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w + 0.3;
    if (frame.debug == 1u) {
        return vec4f(n * 0.5 + 0.5, 1.0);
    }
    if (frame.debug == 2u) {
        // A hue per level, darker through the morph band toward the next coarser level.
        let lambda = lod(in.s);
        return vec4f(hue(ceil(lambda)) * (1.0 - 0.6 * (ceil(lambda) - lambda)) * (0.5 + 0.5 * light), 1.0);
    }
    if (frame.debug == 3u) {
        // Grey only: the red clear colour shows through a crack and nowhere else.
        return vec4f(vec3f(0.25 + 0.5 * light / 1.3), 1.0);
    }
    if (frame.debug == 4u) {
        // Red where drawnHeight leaves the mesh by 5 cm, grey elsewhere. The rounding of the float32
        // position about the eye grows with the distance, so a far pixel on a steep slope can show red.
        let error = abs(drawn_height(in.s) - in.h);
        return select(vec4f(vec3f(0.25 + 0.5 * light / 1.3), 1.0), vec4f(1.0, 0.0, 0.0, 1.0), !(error < 0.05));
    }
    return vec4f(ground_colour(in.s, in.relative, in.h, in.level), 1.0);
}
