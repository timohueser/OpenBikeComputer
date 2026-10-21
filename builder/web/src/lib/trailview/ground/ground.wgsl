// Per ground page compute: when a page arrives, the ground parameters of each texel through its
// region's rules; when it arrives and when its heights change, the openness of each texel. look.wgsl
// reads both filtered, so a pixel never runs the rules.
// #include look.wgsl

struct Job {
    // The node's first texel at its level.
    texel: vec2i,
    level: u32,
    // The ground page.
    slot: u32,
    // Metres per texel at the node's latitude.
    metres: f32,
    // 1: the relief, 2: the ground parameters too.
    work: u32,
    unused: vec2u,
}

@group(0) @binding(0) var<storage, read> jobs: array<Job>;
@group(0) @binding(1) var cache_out: texture_storage_2d_array<rgba8unorm, write>;
// Per job, one layer: the satellite on the date and its NDVI code (0 where no quarter looked). The
// page keeps only what the look reads.
@group(0) @binding(2) var colour_in: texture_2d_array<f32>;
// Per job, one layer: snow by the region's snow_share, snow data known, the satellite's snow mark.
@group(0) @binding(3) var snow_in: texture_2d_array<f32>;

// The height of a texel, or of the coarser levels at its centre where the level does not know it.
fn height_at(level: u32, t: vec2i) -> f32 {
    let h = texel_height(level, t, false);
    if (known(h) || level <= COARSEST) {
        return h;
    }
    let shift = 22u - level;
    return units_height((t << vec2u(shift)) + vec2i(1 << (shift - 1u)), level - 1u);
}

// How open the ground is about a centre height and four heights `reach` metres away: a crease is
// darker and a crest lighter, like ink in a gully. The prototype's deriveFragment, as (open − 0.6) / 0.55.
fn openness(centre: f32, around: vec4f, reach: f32) -> f32 {
    if (!(known(centre) && known(min(min(around.x, around.y), min(around.z, around.w))))) {
        return (1.0 - 0.6) / 0.55;
    }
    return (clamp(1.0 - 1.5 * (0.25 * dot(around, vec4f(1.0)) - centre) / reach, 0.6, 1.15) - 0.6) / 0.55;
}

// The relief of a texel: its openness three texels across, in w.
fn relief_of(job: Job, t: vec2i) -> vec4f {
    let wide = 3;
    let open = openness(height_at(job.level, t), vec4f(height_at(job.level, t + vec2i(wide, 0)), height_at(job.level, t - vec2i(wide, 0)),
        height_at(job.level, t + vec2i(0, wide)), height_at(job.level, t - vec2i(0, wide))), f32(wide) * job.metres);
    return vec4f(0.0, 0.0, 0.0, open);
}

@compute @workgroup_size(16, 16)
fn ground_main(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    let c = vec2i(id.xy);
    let page = i32(job.slot);
    let at = in_page(page, c);
    let layer = page_layer(page);
    textureStore(cache_out, at, 4 * layer + 3, clamp(relief_of(job, job.texel + c), vec4f(0.0), vec4f(1.0)));
    if ((job.work & 2u) == 0u) {
        return;
    }
    let cell = textureLoad(cells, at, layer, 0);
    let sat_texel = textureLoad(colour_in, c, i32(id.z), 0);
    let data = textureLoad(snow_in, c, i32(id.z), 0);
    let sat = satellite(sat_texel);
    let ndvi = ndvi_of(sat_texel);
    let t_ndvi = clamp((ndvi - 0.15) / 0.6, 0.0, 1.0);
    let barren = f32(cell.a) / 255.0;
    let parts = texel_parts(cell, t_ndvi, smoothstep(0.35, 0.6, ndvi), barren, data.g > 0.5 || sat.a > 0.0);
    let region = cell.b;
    // Where the satellite sees green, a steep slope is turf and scrub on the rock up to the region's turf angle at least.
    let green = smoothstep(0.4, 0.7, select(0.0, t_ndvi, sat.a > 0.0)) * (1.0 - parts.bare);
    // Barren ground keeps its own colour on slopes loose cinder and scree hold, up to about 37°; walls
    // past about 47° are rock. Hard rock stands bare from about 37°; soft marl and clay hold scrub on their slopes.
    let bare_from = mix(0.62, 0.766, smoothstep(0.2, 0.6, parts.hardness));
    let dry = 1.0 - parts.water;
    // Woodland is crowns and the shade between them, darker than its leaves.
    let albedo = albedo_of(parts, region, cell.r, sat, barren) * mix(1.0, 0.68, parts.tree);
    textureStore(cache_out, at, 4 * layer, vec4f(sqrt(albedo), (data.r + (1.0 - data.g) * data.b) * dry));
    textureStore(cache_out, at, 4 * layer + 1, vec4f(parts.tree, parts.rough, parts.moor, green));
    textureStore(cache_out, at, 4 * layer + 2, vec4f(mix(bare_from, min(bare_from, rule(region, slot_cliff() + 3).y), green), barren * dry,
        parts.sward * (1.0 - barren * dry), parts.banded));
}
