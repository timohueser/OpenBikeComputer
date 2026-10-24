// Bakes the impostor of a model into its layer of the atlas: the frames of impostor.wgsl, each an
// orthographic view along its direction that fits the model's sphere, with the colour, the cover, the
// normal in the model's frame and the trunk's share of the cover.
// #include draw.wgsl
// #include hemi.wgsl

struct Bake {
    // The model, its radius about (0, 0, 0.5) in model units, and its layer of the atlas.
    model: u32,
    radius: f32,
    layer: u32,
    unused: u32,
}

@group(1) @binding(0) var<uniform> bake: Bake;

struct BakeOut {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) albedo: vec3f,
    // 1 on the trunk, whose vertices carry its top (lifted of draw.wgsl), else 0.
    @location(2) trunk: f32,
}

// Frame `instance` of the bake: an orthographic view along its direction that fits the model's sphere,
// in its cell of the atlas.
@vertex
fn bake_vertex(v: MeshIn, @builtin(instance_index) frame: u32) -> BakeOut {
    let cell = vec2f(f32(frame % 8u), f32(frame / 8u));
    let d = hemi_decode(cell / (FRAMES - 1.0));
    let axes = view_axes(d);
    let m = models[bake.model];
    let model = v.position.xyz * m.extent;
    let p = model - vec3f(0.0, 0.0, 0.5);
    let local = vec2f(dot(p, axes[0]), dot(p, axes[1])) / bake.radius;
    let depth = 0.5 + 0.5 * dot(p, d) / bake.radius;
    var out: BakeOut;
    // The cell's square of the atlas, y down.
    out.clip = vec4f((cell.x + 0.5 + 0.5 * local.x) / FRAMES * 2.0 - 1.0, 1.0 - (cell.y + 0.5 - 0.5 * local.y) / FRAMES * 2.0, depth, 1.0);
    out.normal = v.normal.xyz;
    out.albedo = linear(v.colour.rgb) * crown_shade(model, m);
    out.trunk = select(0.0, 1.0, v.position.w > 0.0);
    return out;
}

struct BakeTexels {
    @location(0) albedo: vec4f,
    @location(1) normal: vec4f,
}

@fragment
fn bake_fragment(in: BakeOut) -> BakeTexels {
    // The normal's alpha holds the trunk times the cover, which the mips average as they do the cover.
    return BakeTexels(vec4f(in.albedo, 1.0), vec4f(normalize(in.normal) * 0.5 + 0.5, in.trunk));
}

