// Per ground page compute: when a page arrives, the ground parameters of each texel through its
// region's rules; when it arrives and when its heights change, the relief of each texel. look.wgsl
// reads both filtered, so a pixel never runs the rules.
// #include look.wgsl
// #include ../vegetation/phenology.wgsl

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
    // The day of the year, 0 for 1 January.
    day: f32,
    unused: u32,
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

// The relief of a texel: the normal and the openness two levels coarser, for cliffs, whose fine
// slopes would streak down a face, and the texel's own openness three texels across.
fn relief_of(job: Job, t: vec2i) -> vec4f {
    let wide = 3;
    let open = openness(height_at(job.level, t), vec4f(height_at(job.level, t + vec2i(wide, 0)), height_at(job.level, t - vec2i(wide, 0)),
        height_at(job.level, t + vec2i(0, wide)), height_at(job.level, t - vec2i(0, wide))), f32(wide) * job.metres);
    // The finest ground pages also draw the nodes finer than them, near the eye, whose LOD is higher.
    let coarse = max(job.level, COARSEST + 2u) - select(2u, 1u, job.level == look.codes.y);
    let shift = 22u - job.level;
    let u = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let n = level_normal(lod_space(vec2f(u - surface.anchor)), coarse);
    let step = wide << (22u - coarse);
    let coarse_open = openness(units_height(u, coarse), vec4f(units_height(u + vec2i(step, 0), coarse), units_height(u - vec2i(step, 0), coarse),
        units_height(u + vec2i(0, step), coarse), units_height(u - vec2i(0, step), coarse)), f32(wide << (job.level - coarse)) * job.metres);
    return vec4f(n * 0.5 + 0.5, coarse_open, open);
}

// The water meshes draw the water. So a texel that the ground layers call water, or where the satellite
// saw water, takes the ground of a dry texel on one of 8 rays within 16 texels of its page, and no blue
// of the layers climbs a shore. The satellite's snow mark is not water: snow has a low NDVI too.
const DRY_STEPS = array<i32, 5>(1, 2, 4, 8, 16);
const DRY_WAYS = array<vec2i, 8>(vec2i(1, 0), vec2i(-1, 0), vec2i(0, 1), vec2i(0, -1), vec2i(1, 1), vec2i(-1, 1), vec2i(1, -1), vec2i(-1, -1));

fn wet(cell: vec4u, sat_texel: vec4f, snow: vec4f) -> bool {
    let cover = u32(rule(cell.b, slot_kinds() + i32(cell.r)).x + 0.5);
    return cover == COVER_WATER || (satellite(sat_texel).a > 0.0 && ndvi_of(sat_texel) < 0.0 && snow.b < 0.5);
}

// The texel of a page whose ground a texel takes: its own, or the first dry one on the rays.
fn dry_texel(page: i32, layer: i32, job: u32, c: vec2i) -> vec2i {
    if (!wet(textureLoad(cells, in_page(page, c), layer, 0), textureLoad(colour_in, c, i32(job), 0), textureLoad(snow_in, c, i32(job), 0))) {
        return c;
    }
    for (var i = 0; i < 5; i++) {
        for (var w = 0; w < 8; w++) {
            let d = clamp(c + DRY_WAYS[w] * DRY_STEPS[i], vec2i(0), vec2i(255));
            if (!wet(textureLoad(cells, in_page(page, d), layer, 0), textureLoad(colour_in, d, i32(job), 0), textureLoad(snow_in, d, i32(job), 0))) {
                return d;
            }
        }
    }
    return c;
}

// The forest's colours per leaf type and elevation (YEAR_STEP_M and YEAR_KNOTS of vegetation/phenology.ts).
const YEAR_STEP_M: f32 = 300.0;
const YEAR_KNOTS: i32 = 10;

// A forest texel on a day: its own colour in the share of its trees in summer, and in the shares of the
// other phases the mean crown colour of the trees of its leaf type at its elevation in that phase
// (forestYear of vegetation/phenology.ts), at its own luma. So the forest past the trees' reach turns
// as they do.
fn forest_on(albedo: vec3f, region: u32, material: u32, elevation: f32, day: f32) -> vec3f {
    let shares = phase_shares(day, elevation);
    let leaf = i32(rule(region, slot_satellite() + i32(material)).y + 0.5);
    let k = clamp(elevation / YEAR_STEP_M, 0.0, f32(YEAR_KNOTS - 1));
    let low = slot_year() + 3 * (leaf * YEAR_KNOTS + i32(k));
    let high = slot_year() + 3 * (leaf * YEAR_KNOTS + min(i32(k) + 1, YEAR_KNOTS - 1));
    let f = fract(k);
    let turned = shares.x * mix(rule(region, low).rgb, rule(region, high).rgb, f) + shares.y * mix(rule(region, low + 1).rgb, rule(region, high + 1).rgb, f)
        + shares.w * mix(rule(region, low + 2).rgb, rule(region, high + 2).rgb, f);
    return shares.z * albedo + luma(albedo) * turned;
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
    let source = dry_texel(page, layer, id.z, c);
    let cell = textureLoad(cells, in_page(page, source), layer, 0);
    let sat_texel = textureLoad(colour_in, source, i32(id.z), 0);
    let data = textureLoad(snow_in, source, i32(id.z), 0);
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
    // Woodland is crowns and the shade between them, darker than its leaves, in the trees' phases.
    var albedo = albedo_of(parts, region, cell.r, sat, barren);
    if (parts.tree > 0.0) {
        let elevation = height_at(job.level, job.texel + source);
        albedo = mix(albedo, forest_on(albedo, region, cell.r, select(REFERENCE_M, elevation, known(elevation)), job.day), parts.tree) * mix(1.0, 0.68, parts.tree);
    }
    textureStore(cache_out, at, 4 * layer, vec4f(sqrt(albedo), (data.r + (1.0 - data.g) * data.b) * dry));
    textureStore(cache_out, at, 4 * layer + 1, vec4f(parts.tree, parts.rough, parts.moor, green));
    textureStore(cache_out, at, 4 * layer + 2, vec4f(mix(bare_from, min(bare_from, rule(region, slot_cliff() + 3).y), green), barren * dry,
        parts.sward * (1.0 - barren * dry), parts.banded));
}
