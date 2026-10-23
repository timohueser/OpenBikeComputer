// The stamp pass: each page that arrives or is synthesized gets its node's stamps before the mesh,
// drawnHeight and the normals read it. A stored page keeps its raw heights for the syntheses below
// it and draws a stamped copy; a synthesized page reads the raw stored pages only, so no level ever
// upsamples a stamped parent. Near pages get the feature mask.
// #include drawn-height.wgsl
// #include stamp-core.wgsl

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

const SYNTHESIZED: u32 = 0xffffffffu;
// The float32 bits of an unknown height in `out`, as in pages.wgsl.
const UNKNOWN32: u32 = 0xffffffffu;
// Modes: stamp the heights, write the mask.
const HEIGHTS: u32 = 1u;
const MASK: u32 = 4u;
// A drawn height this close to a bound of the cones lies on their face: more than the rounding of a
// page's 16-bit heights.
const FACE_M: f32 = 0.02;
// The mask holds distances of ±MASK_RANGE metres in 0.1 m steps, and a track's metres from its middle
// within ±TRACK_RANGE, in 2.5 cm steps for its rails.
const MASK_RANGE: f32 = 12.75;
const TRACK_RANGE: f32 = 3.2;
// Mask flags, as features.wgsl reads them.
const WALL_FLAG: u32 = 1u;
const CONFLICT_FLAG: u32 = 2u;
const FORD_FLAG: u32 = 4u;
// A mesh vertex moves onto a road edge within this many texels of its level: 3/4 of a mesh cell, so
// that on every level with masks each triangle along an edge has a vertex on it.
const SNAP_TEXELS: f32 = 3.0;

// As in pages.wgsl: the ordered bits of a height, so that unsigned atomics take the least and the greatest.
fn ordered(h: f32) -> u32 {
    let b = bitcast<u32>(h);
    return select(b | 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}

fn distance_code(d: f32) -> f32 {
    return clamp(d / (2.0 * MASK_RANGE) + 0.5, 0.0, 1.0);
}

@compute @workgroup_size(8, 8)
fn stamp(@builtin(global_invocation_id) id: vec3u) {
    let job = stamp_jobs[id.z];
    let t = job.first + vec2i(id.xy);
    let shift = 22u - job.level;
    let p = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let s = stamp_at(job.level, job.first, job.data, p);
    var h = NONE;
    if ((job.mode & HEIGHTS) != 0u) {
        let at = job.out * 65536u + id.y * 256u + id.x;
        var base = NONE;
        if (job.base == SYNTHESIZED) {
            let raw = out[at];
            base = select(bitcast<f32>(raw), NONE, raw == UNKNOWN32);
        } else {
            base = slot_height(job.base, t);
        }
        h = select(NONE, clamp(base, s.lo, s.hi), known(base));
        out[at] = select(UNKNOWN32, bitcast<u32>(h), known(h));
        if (known(h)) {
            atomicMin(&extent[2u * job.out], ordered(h));
            atomicMax(&extent[2u * job.out + 1u], ordered(h));
        }
    }
    if ((job.mode & MASK) != 0u) {
        if ((job.mode & HEIGHTS) == 0u) {
            h = slot_height(job.base, t);
        }
        // A cut or a fill face of a road, where the look draws a retaining wall if it is steep.
        let faced = known(h) && ((s.fill && h <= s.lo + FACE_M) || (s.cut && h >= s.hi - FACE_M));
        let flags = select(0u, WALL_FLAG, faced) | select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && s.road < 0.0);
        let surface = select(0u, s.surface, s.road < MASK_RANGE);
        let across = select(distance_code(s.across), clamp(s.across / (2.0 * TRACK_RANGE) + 0.5, 0.0, 1.0), surface >= TRACK_CODE);
        textureStore(mask_out, in_layer(job.mask, t), job.mask >> 6u,
            vec4f(distance_code(s.road), fract(s.sleepers), across, f32(surface | (flags << 4u)) / 255.0));
    }
}

// The move of a mesh vertex of a mask's page onto the nearest road edge, in whole units: from the
// distance to the edge at the four texels about its corner, a quarter texel inside the road, where
// the texels about it hold the road's height. None where a texel holds no road. A vertex on the
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
    var road = true;
    for (var i = 0; i < 4; i++) {
        let texel = textureLoad(mask_in, in_layer(job.mask, low + vec2i(i % 2, i / 2)), job.mask >> 6u, 0);
        road = road && (u32(round(texel.w * 255.0)) & 15u) != 0u;
        d[i] = (texel.x - 0.5) * 2.0 * MASK_RANGE;
    }
    let shift = 22u - job.level;
    let unit = unit_metres(corner.y << shift);
    let metres = unit * f32(1 << shift);
    // The gradient of the distance per metre, and the distance at the corner.
    let g = vec2f(d[1] + d[3] - d[0] - d[2], d[2] + d[3] - d[0] - d[1]) * 0.5 / metres;
    let at = 0.25 * (d[0] + d[1] + d[2] + d[3]) + dot(g, vec2f(corner - low - vec2i(1))) * metres;
    let g2 = dot(g, g);
    let reach_m = SNAP_TEXELS * metres;
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
