// The stamp pass: each page that arrives or is synthesized gets its node's stamps before the mesh,
// drawnHeight and the normals read it. A stored page keeps its raw heights for the syntheses below
// it and draws a stamped copy; a synthesized page reads the raw stored pages only, so no level ever
// upsamples a stamped parent. Near pages get the feature mask.
// #include drawn-height.wgsl
// #include far-stamp.wgsl
// #include mask-walk.wgsl

struct StampJob {
    // The node's first texel at its level.
    first: vec2i,
    level: u32,
    // The node's stamp data in `stamp_data`, or NO_DATA.
    data: u32,
    // The page in `out` that receives the heights, and that holds the synthesized base.
    out: u32,
    // The raw stored page that is the base, or SYNTHESIZED for the base in `out`; for a mask alone,
    // the drawn page.
    base: u32,
    mode: u32,
    mask: u32,
}

@group(0) @binding(0) var<storage, read> stamp_jobs: array<StampJob>;
// Per job, one page of float32 height bits.
@group(0) @binding(2) var<storage, read_write> out: array<u32>;
// Per job, the least and the greatest height as ordered bits, for the offset and the scale of its page.
@group(0) @binding(3) var<storage, read_write> extent: array<atomic<u32>>;
// The feature mask, as features.wgsl reads it.
@group(0) @binding(5) var mask_out: texture_storage_2d_array<rgba8unorm, write>;
// The pass after: the mask that the stamp wrote, and the snap of each mesh vertex of its page, as
// features.wgsl reads it.
@group(0) @binding(6) var mask_in: texture_2d_array<f32>;
@group(0) @binding(7) var snaps: texture_storage_2d_array<r32uint, write>;
// How far the one road model's heights moved the ground per mask texel, as features.wgsl reads it.
@group(0) @binding(10) var moved_out: texture_storage_2d_array<rgba8unorm, write>;
// The moved ground of a workgroup's texels: four of a row go into one texel of moved_out.
var<workgroup> moved_share: array<f32, 64>;

const SYNTHESIZED: u32 = 0xffffffffu;
// The float32 bits of an unknown height in `out`, as in pages.wgsl.
const UNKNOWN32: u32 = 0xffffffffu;
// Modes: stamp the heights, write the mask.
const HEIGHTS: u32 = 1u;
const MASK: u32 = 4u;
// The one road model's far cache of a drawn page past the masks: the look's cache alone, in layer `mask`.
const FAR_CACHE: u32 = 8u;
// A drawn height this close to a bound of the cones lies on their face: more than the rounding of a
// page's 16-bit heights.
const FACE_M: f32 = 0.02;
// The mask holds the distance to a road's edge within ±MASK_RANGE metres on an asinh scale with its knee
// at MASK_KNEE: its steps of 1.5 cm at the edge, where a kerb is 18 cm wide, grow to 0.4 m at MASK_RANGE.
// It holds the metres from a road's middle in 0.1 m steps, and a track's within ±TRACK_RANGE in 2.5 cm
// steps for its rails; else the byte of mask-walk.wgsl.
const MASK_RANGE: f32 = 12.75;
const MASK_KNEE: f32 = 0.5;
const TRACK_RANGE: f32 = 3.2;
// Mask flags, as features.wgsl reads them.
const WALL_FLAG: u32 = 1u;
const CONFLICT_FLAG: u32 = 2u;
const FORD_FLAG: u32 = 4u;
const TAKEN_FLAG: u32 = 8u;
// A mesh vertex moves onto a road edge within this many texels of its level: 3/4 of a mesh cell, so
// that on every level with masks each triangle along an edge has a vertex on it.
const SNAP_TEXELS: f32 = 3.0;
// A vertex outside an edge moves from this far, under a mesh cell: past 3 texels a
// triangle from it reaches the far edge of a path, and rises over its tread.
const SNAP_OUT_TEXELS: f32 = 3.75;
// How far past a way's edge the snap compares the ground, past the bench that a path's stamp holds level.
const UPHILL_TEXELS: f32 = 3.0;
// Where a road's cut or fill meets the ground, the ground rounds over DAYLIGHT_M of height instead of a
// crease. Near the road the slope keeps its crease: the rounding grows from SOFT_FROM_M to SOFT_FULL_M past
// the core.
const DAYLIGHT_M: f32 = 0.8;
const SOFT_FROM_M: f32 = 1.5;
const SOFT_FULL_M: f32 = 4.0;
// As MOVED_RANGE_M of features.wgsl.
const MOVED_RANGE_M: f32 = 0.5;

// As in pages.wgsl: the ordered bits of a height, so that unsigned atomics take the least and the greatest.
fn ordered(h: f32) -> u32 {
    let b = bitcast<u32>(h);
    return select(b | 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}

fn distance_code(d: f32) -> f32 {
    return 0.5 + 0.5 * asinh(clamp(d, -MASK_RANGE, MASK_RANGE) / MASK_KNEE) / asinh(MASK_RANGE / MASK_KNEE);
}

// As mask_metres of features.wgsl.
fn code_distance(code: f32) -> f32 {
    return MASK_KNEE * sinh((2.0 * code - 1.0) * asinh(MASK_RANGE / MASK_KNEE));
}

// The ground before the clamp: a soft minimum with the top of a cut, a soft maximum with the foot of a fill.
fn rounded(base: f32, s: Stamp) -> f32 {
    var h = base;
    if (s.cut) {
        let r = DAYLIGHT_M * smoothstep(SOFT_FROM_M, SOFT_FULL_M, s.cut_x);
        let k = max(r - abs(h - s.hi), 0.0) / max(r, 1e-3);
        h = min(h, s.hi) - k * k * r * 0.25;
    }
    if (s.fill) {
        let r = DAYLIGHT_M * smoothstep(SOFT_FROM_M, SOFT_FULL_M, s.fill_x);
        let k = max(r - abs(h - s.lo), 0.0) / max(r, 1e-3);
        h = max(h, s.lo) + k * k * r * 0.25;
    }
    return h;
}

@compute @workgroup_size(8, 8)
fn stamp(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_id) lane: vec3u) {
    let job = stamp_jobs[id.z];
    let t = job.first + vec2i(id.xy);
    let shift = 22u - job.level;
    let p = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    // The one road model at the texel: the snap moves vertices onto its core's outline, the union of the
    // paved area and the paved shoulder, shoulder and verge layers (a layer no wider than the one inside it
    // is empty, and a path has none).
    // One walk of the records per texel: the label and the core for the heights and the mask, and with the paged
    // paint (probe 2048) the paved label across the nearest edge within two texels. A far layer's texel sees the
    // records as a fragment there does, with model_at's level of detail.
    let texel = unit_metres(p.y) * f32(1 << (22u - job.level));
    let painting = MODEL && (MODEL_PROBE & 2048u) != 0u;
    let far_layer = (job.mode & FAR_CACHE) != 0u;
    let mp = model_at_lod(p, vec2f(0.0), select(0.0, 2.0 * texel, painting && (far_layer || (job.mode & MASK) != 0u)), M_FAR, far_layer);
    // Where the model holds the point, its heights take the place of the far stamp's roads; the stamp's
    // water and structures still shape the ground under them. Probe 16384 keeps the far stamp's heights.
    let heights = MODEL && mp.block != 0u && (MODEL_PROBE & 16384u) == 0u;
    var s: Stamp;
    var water = vec2f(-INF, INF);
    var slope = vec2f(-INF, INF);
    if (FAR_STAMP) {
        let fs = far_stamp_at(job.level, job.first, job.data, p, !heights);
        s = fs.stamp;
        water = fs.water;
        slope = fs.slope;
    } else {
        s = stamp_at(job.level, job.first, job.data, p);
    }
    let core = min(min(mp.d_paved, mp.d_band[0]), min(mp.d_band[1], mp.d_band[2]));
    let modelled = MODEL && mp.block != 0u && core < M_FAR;
    var h = NONE;
    var sloped = false;
    var moved = 0.0;
    // The ground that the roads shape: under the model the plain terrain after the water and the structures, which
    // height_at takes; else the plain terrain.
    var ground = NONE;
    if ((job.mode & HEIGHTS) != 0u) {
        let at = job.out * 65536u + id.y * 256u + id.x;
        var base = NONE;
        if (job.base == SYNTHESIZED) {
            let raw = out[at];
            base = select(bitcast<f32>(raw), NONE, raw == UNKNOWN32);
        } else {
            base = slot_height(job.base, t);
        }
        h = select(NONE, clamp(rounded(base, s), s.lo, s.hi), known(base));
        ground = select(base, h, heights);
        // The one road model's heights: a point function of the same records as the shapes, over the
        // ground that the water and the structures shaped. A water's core bounds them; off the paved
        // area a bore's cover and a deck's underside do too, so a road runs on onto its deck.
        if (heights && known(base)) {
            let hk = height_at(mp.block, mp.q, mp.label, h, mp.heights);
            h = clamp(select(hk.x, clamp(hk.x, slope.x, slope.y), hk.y != H_PAVED), water.x, max(water.x, water.y));
            // A slope's cone is no wall; the face of a wall of the heights is (the no-room rule).
            sloped = hk.y > 2.5;
        }
        out[at] = select(UNKNOWN32, bitcast<u32>(h), known(h));
        if (known(h)) {
            atomicMin(&extent[2u * job.out], ordered(h));
            atomicMax(&extent[2u * job.out + 1u], ordered(h));
        }
    }
    if (MODEL && far_layer) {
        if (painting) {
            textureStore(model_paged_out, in_layer(job.mask, t), job.mask >> 6u, paint_texel(mp, s));
        } else {
            textureStore(model_cache_out, in_layer(job.mask, t), job.mask >> 6u, model_cache_texel(p, texel, mp.d_band));
        }
    }
    if ((job.mode & MASK) != 0u) {
        if ((job.mode & HEIGHTS) == 0u) {
            h = slot_height(job.base, t);
            // A late mask of a synthesized page: its plain terrain is the bicubic that synthesized it.
            let plain = stored_base(p, job.level);
            ground = select(plain, select(NONE, clamp(rounded(plain, s), s.lo, s.hi), known(plain)), heights);
        }
        moved = select(0.0, abs(h - ground), known(h) && known(ground));
        // A cut or a fill face of a road, where the look draws a retaining wall if it is steep.
        let faced = select(known(h) && ((s.fill && h <= s.lo + FACE_M) || (s.cut && h >= s.hi - FACE_M)), sloped, MODEL && mp.block != 0u);
        let flags = select(0u, WALL_FLAG, faced) | select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && s.road < 0.0) | select(0u, TAKEN_FLAG, s.taken);
        var surface = select(0u, s.surface, s.road < MASK_RANGE);
        // A way of the model that the far stamp does not draw still marks its texels.
        if (modelled && surface == 0u && core < MASK_RANGE) {
            surface = 1u;
        }
        if (painting) {
            textureStore(model_paged_out, in_layer(job.mask, t), job.mask >> 6u, paint_texel(mp, s));
        }
        // The metres across and the distance of a texel belong to one road: the model's distance to its core
        // takes the far stamp's metres from the middle to its edge on top, so the half-width that the look
        // and the snap read (|across| less the distance) is the far stamp's road's, never the difference
        // of two roads. A way of the model alone counts as a road.
        let beyond = select(MASK_RANGE, max(abs(s.across) - s.road, 0.0), s.road < MASK_RANGE);
        let own = select(s.across, select(-1.0, 1.0, s.across >= 0.0) * min(core + beyond, MASK_RANGE), modelled);
        let across = clamp(own / (2.0 * select(MASK_RANGE, TRACK_RANGE, surface >= TRACK_CODE)) + 0.5, 0.0, 1.0);
        let walk = f32(walk_byte(s.walk, s.shoulder && !modelled, s.trodden, s.marked)) / 255.0;
        let road = select(s.road, core, modelled);
        textureStore(mask_out, in_layer(job.mask, t), job.mask >> 6u,
            vec4f(distance_code(road), select(walk, fract(s.sleepers), surface >= TRACK_CODE), across, f32(surface | (flags << 4u)) / 255.0));
        if (MODEL && (MODEL_PROBE & 2097152u) != 0u) {
            textureStore(model_cache_out, in_layer(job.mask, t), job.mask >> 6u, model_cache_texel(p, texel, mp.d_band));
        }
    }
    moved_share[lane.y * 8u + lane.x] = clamp(moved / MOVED_RANGE_M, 0.0, 1.0);
    workgroupBarrier();
    if (MODEL && (job.mode & MASK) != 0u && (lane.x & 3u) == 0u) {
        let k = lane.y * 8u + lane.x;
        let at = in_layer(job.mask, t);
        textureStore(moved_out, vec2i(at.x >> 2u, at.y), job.mask >> 6u, vec4f(moved_share[k], moved_share[k + 1u], moved_share[k + 2u], moved_share[k + 3u]));
    }
}

// The paged paint of a texel (probe 2048) in the mask's format, read by mask_in of features.wgsl: the distance to the
// paved outline, the walk byte or a track's sleepers, the metres across from the host's middle, and the finish's code
// (0 none) of the paved label there, else of the one across the nearest change within the reach of `mp`.
fn paint_texel(mp: ModelPoint, s: Stamp) -> vec4f {
    let id = select(mp.alt, mp.label, model_surface(mp.block, mp.label) == S_CARRIAGEWAY);
    let finish = model_finish(mp.block, id) & 0xffu;
    let code = select(0u, select(finish, 1u, finish == 2u), mp.block != 0u && mp.d_paved < MASK_RANGE && finish != 0u && id != M_NONE);
    let track = code >= TRACK_CODE;
    let across = clamp(select(mp.across, MASK_RANGE, mp.across == M_FAR) / (2.0 * select(MASK_RANGE, TRACK_RANGE, track)) + 0.5, 0.0, 1.0);
    let walk = select(f32(walk_byte(s.walk, false, s.trodden, s.marked)) / 255.0, fract(s.sleepers), track);
    return vec4f(distance_code(mp.d_paved), walk, across, f32(code) / 255.0);
}

// The rise per metre of a value at the four texels about a vertex's corner.
fn rise_of(c: array<f32, 4>, metres: f32) -> vec2f {
    return vec2f(c[1] + c[3] - c[0] - c[2], c[2] + c[3] - c[0] - c[1]) * 0.5 / metres;
}

// The value at the corner, `past` metres from the middle of the four texels.
fn corner_value(c: array<f32, 4>, rise: vec2f, past: vec2f) -> f32 {
    return 0.25 * (c[0] + c[1] + c[2] + c[3]) + dot(rise, past);
}

// The height that a job's page holds at a texel, inside the page.
fn page_height(job: StampJob, t: vec2i) -> f32 {
    let p = clamp(t, job.first, job.first + vec2i(255));
    if ((job.mode & HEIGHTS) == 0u) {
        return slot_height(job.base, p);
    }
    let v = vec2u(p - job.first);
    let raw = out[job.out * 65536u + v.y * 256u + v.x];
    return select(bitcast<f32>(raw), NONE, raw == UNKNOWN32);
}

// The byte of mask-walk.wgsl at a mask texel. A track holds its sleepers there.
fn walk_code(texel: vec4f) -> u32 {
    return select(u32(round(texel.y * 255.0)), 0u, (u32(round(texel.w * 255.0)) & 15u) >= TRACK_CODE);
}

// The sign of the metres across on the side of a way where the ground stands higher, `reach` metres from its middle.
// A vertex lies `middle` metres across at `corner`.
fn higher_side(job: StampJob, corner: vec2i, rise: vec2f, middle: f32, reach: f32, metres: f32) -> f32 {
    let unit = normalize(rise) / metres;
    let left = page_height(job, corner + vec2i(round(unit * (reach - middle))));
    let right = page_height(job, corner - vec2i(round(unit * (reach + middle))));
    return select(-1.0, 1.0, left >= right);
}

// The move of a mesh vertex of a mask's page onto the outer edge of a carriageway's shoulder, or of its
// sidewalk where it has one, in whole units: from the distance to the edge at the four texels about its corner, a quarter
// texel inside, where the texels about it hold the road's height. The ground is flat across a sidewalk or a
// shoulder, so its outer edge is the ground's only crease there. None where a texel holds no road. A vertex on the
// lower or the left edge of the page reads the four texels beside it inside the page, so that no page
// reads another. A vertex on an edge of its node moves only along it: every border between two
// levels lies on an edge of the coarser node, so each node draws exactly its own area.
@compute @workgroup_size(8, 8)
fn snap(@builtin(global_invocation_id) id: vec3u) {
    let job = stamp_jobs[id.z];
    if ((job.mode & MASK) == 0u) {
        return;
    }
    let v = vec2i(id.xy);
    let corner = job.first + 4 * v;
    let low = max(corner - vec2i(1), job.first);
    var d: array<f32, 4>;
    var across: array<f32, 4>;
    var road = true;
    var walk = 0.0;
    var shoulder = 0.0;
    for (var i = 0; i < 4; i++) {
        let texel = textureLoad(mask_in, in_layer(job.mask, low + vec2i(i % 2, i / 2)), job.mask >> 6u, 0);
        let surface = u32(round(texel.w * 255.0)) & 15u;
        road = road && surface != 0u;
        d[i] = code_distance(texel.x);
        across[i] = (texel.z - 0.5) * 2.0 * select(MASK_RANGE, TRACK_RANGE, surface >= TRACK_CODE);
        let wide = walk_code(texel);
        walk = max(walk, walk_metres(wide));
        shoulder = max(shoulder, select(0.0, shoulder_of(surface), holds_shoulder(wide)));
    }
    let shift = 22u - job.level;
    let unit = unit_metres(corner.y << shift);
    let metres = unit * f32(1 << shift);
    // The gradient of the distance per metre, and the distance at the corner.
    let past = vec2f(corner - low - vec2i(1)) * metres;
    var g = rise_of(d, metres);
    var to_edge = corner_value(d, g, past);
    let outer = select(shoulder, walk, walk > 0.0);
    // A cell's fixed diagonal can join a vertex inside a way to one past its edge, and a way narrower than that diagonal
    // cannot hold a vertex on each edge. On the higher edge such a triangle would rise over the face, so a vertex that the
    // distance sends elsewhere, or that lies on the distance's ridge on the way's middle, goes there if it can. Only the
    // metres across go on through that ridge, but their 0.1 m steps miss an edge that the distance finds to 1.5 cm, so
    // only such a vertex takes them. Under a lower edge the triangle stays hidden. `walk` is the width on the vertex's
    // side, so a way whose far side differs keeps the distance.
    let rise = rise_of(across, metres);
    let near = dot(g, g);
    if (road && to_edge < outer && dot(rise, rise) >= 0.25 && dot(rise, rise) <= 4.0) {
        let middle = corner_value(across, rise, past);
        let half = abs(across[0]) - d[0];
        let side = higher_side(job, corner, rise, middle, half + outer + UPHILL_TEXELS * metres, metres);
        let toward = side * middle;
        if ((toward < 0.0 || near < 0.25 || near > 4.0) && toward - half - outer >= -SNAP_TEXELS * metres) {
            let far = corner + vec2i(round(normalize(rise) * (side * (half - 0.5 * metres) - middle) / metres));
            let far_texel = textureLoad(mask_in, in_layer(job.mask, clamp(far, job.first, job.first + vec2i(255))), job.mask >> 6u, 0);
            if (walk_metres(walk_code(far_texel)) == walk) {
                g = side * rise;
                to_edge = toward - half;
            }
        }
    }
    let at = to_edge - outer;
    let g2 = dot(g, g);
    let reach_m = select(SNAP_TEXELS, SNAP_OUT_TEXELS, at > 0.0) * metres;
    var moved = vec2i(0);
    // Off the ridges of the distance, where it turns from one edge to another.
    if (road && g2 >= 0.25 && g2 <= 4.0 && abs(at) <= reach_m) {
        // A distance grows by a metre per metre, so the step takes only the direction of the quantized
        // gradient: a step scaled by the gradient's length overshoots or falls short, and the triangles
        // fold on the edge.
        let shift_m = -(at + 0.25 * metres) * g / sqrt(g2);
        moved = vec2i(round(shift_m * min(1.0, reach_m / length(shift_m)) / unit));
        moved = select(moved, vec2i(0), v == vec2i(0));
    }
    textureStore(snaps, v, job.mask, vec4u((bitcast<u32>(moved.x) & 0xffffu) | (bitcast<u32>(moved.y) << 16u)));
}
