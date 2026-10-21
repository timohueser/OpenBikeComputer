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
    // The raw stored page that is the base, or SYNTHESIZED for the base in `out`.
    base: u32,
    mode: u32,
    mask: u32,
}

@group(0) @binding(0) var<storage, read> stamp_jobs: array<StampJob>;
// Per job, one page of float32 height bits.
@group(0) @binding(2) var<storage, read_write> out: array<u32>;
// Per job, the least and the greatest height as ordered bits, for the offset and the scale of its page.
@group(0) @binding(3) var<storage, read_write> extent: array<atomic<u32>>;
@group(0) @binding(4) var mask_shape: texture_storage_2d_array<rgba8unorm, write>;
@group(0) @binding(5) var mask_kind: texture_storage_2d_array<rgba8uint, write>;

const SYNTHESIZED: u32 = 0xffffffffu;
// The float32 bits of an unknown height in `out`, as in pages.wgsl.
const UNKNOWN32: u32 = 0xffffffffu;
// Modes: stamp the heights, as the bake would (every feature, nominal margins), write the mask.
const HEIGHTS: u32 = 1u;
const BAKED: u32 = 2u;
const MASK: u32 = 4u;
// The mask holds distances of ±MASK_RANGE metres in 0.1 m steps.
const MASK_RANGE: f32 = 12.75;
// Mask flags, as features.wgsl reads them.
const CONFLICT_FLAG: u32 = 4u;
const FORD_FLAG: u32 = 8u;

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
    let at = job.out * 65536u + id.y * 256u + id.x;
    var base = NONE;
    if (job.base == SYNTHESIZED) {
        let raw = out[at];
        base = select(bitcast<f32>(raw), NONE, raw == UNKNOWN32);
    } else {
        base = slot_height(job.base, t);
    }
    let s = stamp_at(job.level, job.first, job.data, p, (job.mode & BAKED) != 0u);
    let h = select(NONE, clamp(base, s.lo, s.hi), known(base));
    if ((job.mode & HEIGHTS) != 0u) {
        out[at] = select(UNKNOWN32, bitcast<u32>(h), known(h));
        if (known(h)) {
            atomicMin(&extent[2u * job.out], ordered(h));
            atomicMax(&extent[2u * job.out + 1u], ordered(h));
        }
    }
    if ((job.mode & MASK) != 0u) {
        let cut = select(0.0, h - base, known(base));
        let flags = select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && s.road < 0.0);
        textureStore(mask_shape, in_layer(job.mask, t), job.mask >> 6u,
            vec4f(distance_code(s.road), distance_code(s.shore), s.across * 0.5 + 0.5, distance_code(cut)));
        textureStore(mask_kind, in_layer(job.mask, t), job.mask >> 6u,
            vec4u(select(0u, s.surface, s.road < MASK_RANGE), select(0u, 1u, s.water > -1e29), flags, 0u));
    }
}
