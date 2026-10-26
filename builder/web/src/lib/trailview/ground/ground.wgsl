// Per ground page compute: when a page arrives, the ground parameters of each texel through its
// region's rules; when it arrives and when its heights change, the relief of each texel and the
// horizons. look.wgsl reads them filtered, so a pixel never runs the rules nor marches.
// #include look.wgsl

struct Job {
    // The node's first texel at its level.
    texel: vec2i,
    level: u32,
    // The ground page.
    slot: u32,
    // Metres per texel at the node's latitude.
    metres: f32,
    // Bits: 1 the relief, 2 the ground parameters, 4 the horizons.
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
@group(0) @binding(4) var horizons_out: texture_storage_2d_array<rgba8unorm, write>;
// Per job, the openness of each texel as one byte, four per word, for a copy into `relief_open`.
@group(0) @binding(5) var<storage, read_write> open_out: array<atomic<u32>>;
@group(0) @binding(6) var coarse_out: texture_storage_2d_array<rgba8unorm, write>;

// The march of a horizon: from two texels out to HORIZON_M, each step HORIZON_STEP times the last
// and at least a texel, on the level whose texel is about as long as the step. The far terrain is
// the coarse cover of the pages, which lies in every direction about the eye.
const HORIZON_M: f32 = 50000.0;
const HORIZON_STEP: f32 = 1.1;
// CURVATURE of frame.ts: the Earth's drop with refraction, which the drawn terrain takes too.
const CURVATURE: f32 = 6.8278e-8;

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

// Texels across which the relief measures the openness.
const WIDE: i32 = 3;

// A texel's own openness.
fn open_of(job: Job, t: vec2i) -> f32 {
    return openness(height_at(job.level, t), vec4f(height_at(job.level, t + vec2i(WIDE, 0)), height_at(job.level, t - vec2i(WIDE, 0)),
        height_at(job.level, t + vec2i(0, WIDE)), height_at(job.level, t - vec2i(0, WIDE))), f32(WIDE) * job.metres);
}

// The relief two levels coarser at whole units u, for cliffs, whose fine slopes would streak down a face:
// xy the normal as 0.5 + 0.5 n, z the openness.
fn coarse_of(job: Job, u: vec2i) -> vec3f {
    // The finest ground pages also draw the nodes finer than them, near the eye, whose LOD is higher.
    let coarse = max(job.level, COARSEST + 2u) - select(2u, 1u, job.level == look.codes.y);
    let n = level_normal(lod_space(vec2f(u - surface.anchor)), coarse);
    let step = WIDE << (22u - coarse);
    let coarse_open = openness(units_height(u, coarse), vec4f(units_height(u + vec2i(step, 0), coarse), units_height(u - vec2i(step, 0), coarse),
        units_height(u + vec2i(0, step), coarse), units_height(u - vec2i(0, step), coarse)), f32(WIDE << (job.level - coarse)) * job.metres);
    return vec3f(n * 0.5 + 0.5, coarse_open);
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

@compute @workgroup_size(16, 16)
fn ground_main(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    if ((job.work & 1u) == 0u) {
        return;
    }
    let c = vec2i(id.xy);
    let page = i32(job.slot);
    let at = in_page(page, c);
    let layer = page_layer(page);
    let open = clamp(open_of(job, job.texel + c), 0.0, 1.0);
    atomicOr(&open_out[id.z * 16384u + id.y * 64u + id.x / 4u], u32(round(open * 255.0)) << (8u * (id.x % 4u)));
    // The first lanes also make the coarse relief, one texel each at the centre of its texels.
    if (all(c < vec2i(COARSE_PAGE))) {
        let coarse = coarse_of(job, (job.texel + COARSE_STEP * c + COARSE_STEP / 2) << vec2u(22u - job.level));
        textureStore(coarse_out, in_coarse(page, c), layer, vec4f(clamp(coarse, vec3f(0.0), vec3f(1.0)), 0.0));
    }
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
    // Woodland is crowns and the shade between them, darker than its leaves.
    let albedo = albedo_of(parts, region, cell.r, sat, barren) * mix(1.0, 0.68, parts.tree);
    textureStore(cache_out, at, CACHE_PLANES * layer, vec4f(sqrt(albedo), (data.r + (1.0 - data.g) * data.b) * dry));
    textureStore(cache_out, at, CACHE_PLANES * layer + 1, vec4f(parts.tree, parts.rough, parts.moor, green));
    textureStore(cache_out, at, CACHE_PLANES * layer + 2, vec4f(mix(bare_from, min(bare_from, rule(region, slot_cliff() + 3).y), green), barren * dry,
        parts.sward * (1.0 - barren * dry), parts.banded));
}

// The horizons of a page: per horizon texel, from the centre of its texels, the highest altitude of the
// drawn terrain in each bearing. Lane z is a job and a layer of 4 bearings.
@compute @workgroup_size(8, 8)
fn horizon_main(@builtin(global_invocation_id) id: vec3u) {
    let layers = BEARINGS / 4u;
    let job = jobs[id.z / layers];
    if ((job.work & 4u) == 0u) {
        return;
    }
    let layer = id.z % layers;
    let shift = 22u - job.level;
    let u = (job.texel + HORIZON_SPAN * vec2i(id.xy) + HORIZON_SPAN / 2) << vec2u(shift);
    let h0 = units_height(u, job.level);
    let unit = job.metres / f32(1 << shift);
    var out = vec4f(0.0);
    for (var k = 0u; k < 4u && known(h0); k++) {
        let bearing = f32(4u * layer + k) / f32(BEARINGS) * 6.2831853;
        // Units run south.
        let way = vec2f(sin(bearing), -cos(bearing)) / unit;
        var rise = 0.0;
        for (var d = 2.0 * job.metres; d < HORIZON_M; d += max((HORIZON_STEP - 1.0) * d, job.metres)) {
            let coarser = u32(max(log2(d * (HORIZON_STEP - 1.0) / job.metres), 0.0));
            let h = units_height(u + vec2i(round(way * d)), job.level - min(coarser, job.level - COARSEST));
            if (known(h)) {
                rise = max(rise, (h - h0 - CURVATURE * d * d) / d);
            }
        }
        out[k] = atan(rise) / HORIZON_TOP;
    }
    let page = i32(job.slot);
    textureStore(horizons_out, in_horizon(page, vec2i(id.xy)), i32(layers) * page_layer(page) + i32(layer), out);
}
