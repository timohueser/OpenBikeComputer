// Per ground page compute: how open the ground is, from the heights of the page's level, and for the
// ground lab `cache` the ground parameters of each texel, which look.wgsl then reads filtered.
// #include look.wgsl

struct Job {
    // The node's first texel at its level.
    texel: vec2i,
    level: u32,
    // The ground page.
    slot: u32,
    // Metres per texel at the node's latitude.
    metres: f32,
    // 1: openness, 2: the parameter cache.
    work: u32,
    unused: vec2u,
}

@group(0) @binding(0) var<storage, read> jobs: array<Job>;
// Per job, one page of openness, four texels per word.
@group(0) @binding(1) var<storage, read_write> open_out: array<u32>;
@group(0) @binding(2) var cache_out: texture_storage_2d_array<rgba8unorm, write>;

// The height of a texel, or of the coarser levels at its centre where the level does not know it.
fn height_at(level: u32, t: vec2i) -> f32 {
    let h = texel_height(level, t, false);
    if (known(h) || level <= COARSEST) {
        return h;
    }
    let shift = 22u - level;
    return units_height((t << vec2u(shift)) + vec2i(1 << (shift - 1u)), level - 1u);
}

// How open the ground is a few texels across: a crease is darker and a crest lighter, like ink in a
// gully. The prototype's deriveFragment, as (open − 0.6) / 0.55.
fn openness_of(job: Job, t: vec2i) -> f32 {
    let wide = 3;
    let centre = height_at(job.level, t);
    let around = vec4f(height_at(job.level, t + vec2i(wide, 0)), height_at(job.level, t - vec2i(wide, 0)),
        height_at(job.level, t + vec2i(0, wide)), height_at(job.level, t - vec2i(0, wide)));
    if (!(known(centre) && known(min(min(around.x, around.y), min(around.z, around.w))))) {
        return (1.0 - 0.6) / 0.55;
    }
    let open = clamp(1.0 - 1.5 * (0.25 * dot(around, vec4f(1.0)) - centre) / (f32(wide) * job.metres), 0.6, 1.15);
    return (open - 0.6) / 0.55;
}

@compute @workgroup_size(16, 16)
fn ground_main(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    // Four texels in a row per invocation.
    let c = vec2i(i32(id.x) * 4, i32(id.y));
    if ((job.work & 1u) != 0u) {
        var word = 0u;
        for (var i = 0; i < 4; i++) {
            word |= u32(round(255.0 * clamp(openness_of(job, job.texel + c + vec2i(i, 0)), 0.0, 1.0))) << (8u * u32(i));
        }
        open_out[(id.z * 256u + id.y) * 64u + id.x] = word;
    }
    if ((job.work & 2u) == 0u) {
        return;
    }
    let page = i32(job.slot);
    let layer = page_layer(page);
    for (var i = 0; i < 4; i++) {
        let at = in_page(page, c + vec2i(i, 0));
        let cell = textureLoad(cells, at, layer, 0);
        let sat_texel = textureLoad(colour, at, layer, 0);
        let data = textureLoad(snow_state, at, layer, 0);
        let sat = satellite(sat_texel);
        let ndvi = ndvi_of(sat_texel);
        let barren = f32(cell.a) / 255.0;
        // Zeroed for each texel: SwiftShader does not zero a declaration in a loop body again.
        var parts = Parts();
        add_texel(&parts, cell, 1.0, clamp((ndvi - 0.15) / 0.6, 0.0, 1.0), smoothstep(0.35, 0.6, ndvi), barren, data.g > 0.5 || sat.a > 0.0);
        let albedo = albedo_of(parts, cell.b, cell.r, sat, barren);
        textureStore(cache_out, at, 4 * layer, vec4f(sqrt(albedo), parts.water));
        textureStore(cache_out, at, 4 * layer + 1, vec4f(sqrt(parts.stone), parts.banded));
        textureStore(cache_out, at, 4 * layer + 2, vec4f(parts.tree, parts.rough, parts.moor, parts.bare));
        textureStore(cache_out, at, 4 * layer + 3, vec4f(barren * (1.0 - parts.water), parts.sward, 0.0, parts.hardness));
    }
}
