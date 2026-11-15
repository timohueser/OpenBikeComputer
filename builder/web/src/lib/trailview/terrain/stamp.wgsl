// The stamp pass: each page that arrives or is synthesized gets its node's stamps before the mesh,
// drawnHeight and the normals read it. A stored page keeps its raw heights for the syntheses below
// it and draws a stamped copy; a synthesized page reads the raw stored pages only, so no level ever
// upsamples a stamped parent. Near pages get the feature mask. With MODEL the records of the cells
// shape and paint the ways; today's stamps then hold only the water and the structures.
// #include drawn-height.wgsl
// #include stamp-core.wgsl
// #include mask-walk.wgsl
// #include model.wgsl

struct StampJob {
    // The node's first texel at its level.
    first: vec2i,
    level: u32,
    // The node's stamp data in `stamp_data`, or NO_DATA.
    data: u32,
    // The page in `out` that receives the heights, and that holds the synthesized base.
    out: u32,
    // The raw stored page that is the base, or SYNTHESIZED for the base in `out`; for a mask alone,
    // the drawn page, or SYNTHESIZED for a page synthesized in the frame: its layer gets the heights in
    // `out` only after the stamps.
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
// With MODEL, per mask texel: the paint of the paved label (paint_texel) and how far the heights moved
// the ground, four texels of a row in one texel, as features.wgsl reads them.
@group(0) @binding(9) var paint_out: texture_storage_2d_array<rgba8unorm, write>;
@group(0) @binding(10) var moved_out: texture_storage_2d_array<rgba8unorm, write>;
var<workgroup> moved_share: array<f32, 64>;

const SYNTHESIZED: u32 = 0xffffffffu;
// The float32 bits of an unknown height in `out`, as in pages.wgsl.
const UNKNOWN32: u32 = 0xffffffffu;
// Modes: stamp the heights, write the mask.
const HEIGHTS: u32 = 1u;
const MASK: u32 = 4u;
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
// As MOVED_RANGE_M of features.wgsl.
const MOVED_RANGE_M: f32 = 0.5;
// The paint finds the paved label across a change this many texels away.
const PAINT_TEXELS: f32 = 2.0;

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

@compute @workgroup_size(8, 8)
fn stamp(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_id) lane: vec3u) {
    let job = stamp_jobs[id.z];
    let t = job.first + vec2i(id.xy);
    let shift = 22u - job.level;
    let p = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let s = stamp_at(job.level, job.first, job.data, p);
    var mp = ModelPoint(0u, vec2i(0), M_NONE, M_NONE, M_FAR, M_FAR, M_FAR, 0.0, 0.0, 0u, M_NONE);
    if (MODEL && job.level >= M_CELL_LEVEL) {
        mp = model_at(p, PAINT_TEXELS * unit_metres(p.y) * f32(1 << shift));
    }
    var h = NONE;
    // The ground that the ways shape: the plain terrain after the water and the structures.
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
        ground = select(NONE, stamped_ground(base, s), known(base));
        h = model_height(mp, ground, s);
        out[at] = select(UNKNOWN32, bitcast<u32>(h), known(h));
        if (known(h)) {
            atomicMin(&extent[2u * job.out], ordered(h));
            atomicMax(&extent[2u * job.out + 1u], ordered(h));
        }
    }
    if ((job.mode & MASK) != 0u) {
        if ((job.mode & HEIGHTS) == 0u) {
            h = page_height(job, t);
            // A late mask: the plain terrain is the bicubic that made the page.
            if (MODEL) {
                let plain = stored_base(p, job.level);
                ground = select(NONE, stamped_ground(plain, s), known(plain));
            }
        }
        if (MODEL) {
            textureStore(mask_out, in_layer(job.mask, t), job.mask >> 6u, model_mask(mp, s));
            textureStore(paint_out, in_layer(job.mask, t), job.mask >> 6u, paint_texel(mp));
        } else {
            // A cut or a fill face of a road, where the look draws a retaining wall if it is steep.
            let faced = known(h) && ((s.fill && h <= s.lo + FACE_M) || (s.cut && h >= s.hi - FACE_M));
            let flags = select(0u, WALL_FLAG, faced) | select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && s.road < 0.0) | select(0u, TAKEN_FLAG, s.taken);
            let surface = select(0u, s.surface, s.road < MASK_RANGE);
            let across = clamp(s.across / (2.0 * select(MASK_RANGE, TRACK_RANGE, surface >= TRACK_CODE)) + 0.5, 0.0, 1.0);
            let walk = f32(walk_byte(s.walk, s.shoulder, s.trodden, s.marked)) / 255.0;
            textureStore(mask_out, in_layer(job.mask, t), job.mask >> 6u,
                vec4f(distance_code(s.road), select(walk, fract(s.sleepers), surface >= TRACK_CODE), across, f32(surface | (flags << 4u)) / 255.0));
        }
    }
    if (MODEL) {
        moved_share[lane.y * 8u + lane.x] = select(0.0, clamp(abs(h - ground) / MOVED_RANGE_M, 0.0, 1.0), known(h) && known(ground));
        workgroupBarrier();
        if ((job.mode & MASK) != 0u && (lane.x & 3u) == 0u) {
            let k = lane.y * 8u + lane.x;
            let at = in_layer(job.mask, t);
            textureStore(moved_out, vec2i(at.x >> 2u, at.y), job.mask >> 6u, vec4f(moved_share[k], moved_share[k + 1u], moved_share[k + 2u], moved_share[k + 3u]));
        }
    }
}

// The height of the records at a texel over the ground that the water and the structures shaped. A
// water's core and, off the paved area, a bore's cover and a deck's underside bound it, so a road runs
// on onto its deck.
fn model_height(mp: ModelPoint, ground: f32, s: Stamp) -> f32 {
    if (!MODEL || mp.block == 0u || !known(ground)) {
        return ground;
    }
    let hk = height_at(mp.block, vec2f(mp.q), mp.label, ground, mp.heights);
    return stamp_bounded(hk.x, hk.y == H_PAVED, s);
}

// The finish of the painted label; in a stop arm's mouth, the arm's own.
fn painted_finish(mp: ModelPoint) -> u32 {
    return select(model_finish(mp.block, mp.painted), m_line_finish(mp.block, mp.mouth), mp.mouth != M_NONE);
}

// The feature mask of a texel from the records: the distance to the core's outline, which every reader
// takes for a way's edge; the metres across of the way whose core it is, so that |across| less the
// distance is its half-width; a way of the records counts as surface code 1. The water and the
// structures keep their flags, and the records' water face takes the texel, so no plant stands in it.
fn model_mask(mp: ModelPoint, s: Stamp) -> vec4f {
    let road = mp.block != 0u && mp.core < MASK_RANGE;
    let water = mp.block != 0u && mp.label != M_NONE && m_surface(mp.block, mp.label) == M_SURFACE_WATER;
    let flags = select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && mp.core < 0.0) | select(0u, TAKEN_FLAG, s.taken || water);
    let side = select(1.0, sign(mp.across), mp.across != M_FAR && mp.across != 0.0);
    let half = select(mp.half, 0.0, mp.across == M_FAR);
    let across = clamp(side * min(max(mp.core, 0.0) + half, MASK_RANGE) / (2.0 * MASK_RANGE) + 0.5, 0.0, 1.0);
    let walk = f32(walk_byte(0.0, false, (painted_finish(mp) & 0xffu) == 6u, false)) / 255.0;
    return vec4f(distance_code(mp.core), walk, across, f32(select(0u, 1u, road) | (flags << 4u)) / 255.0);
}

// The paint of a texel in the mask's format, read by mask_in of features.wgsl: the distance to the paved
// outline, the walk byte (or a track's sleepers), the metres across from the host's middle (a road's on the
// distance's asinh scale), and the finish code of the painted label (0 none) in the low 4 bits of a byte with
// its top's material (sections::Material, at most 15) above them. A road of a lesser class paints in the main road's asphalt.
fn paint_texel(mp: ModelPoint) -> vec4f {
    let finish = painted_finish(mp);
    var code = select(finish & 0xffu, 1u, (finish & 0xffu) == 2u);
    if (mp.block == 0u || mp.painted == M_NONE || mp.d_paved >= MASK_RANGE) {
        code = 0u;
    }
    let track = code >= TRACK_CODE;
    let metres = select(mp.across, MASK_RANGE, mp.across == M_FAR);
    let across = select(distance_code(metres), clamp(metres / (2.0 * TRACK_RANGE) + 0.5, 0.0, 1.0), track);
    // Lines on the asphalt of a way that wears them (the records' flag); a soil path is trodden.
    let marked = (finish & M_MARKED) != 0u;
    let walk = f32(walk_byte(0.0, false, code == 6u, marked)) / 255.0;
    let material = select(0u, (finish >> M_MATERIAL_SHIFT) & 15u, code != 0u);
    return vec4f(distance_code(mp.d_paved), select(walk, fract(mp.along / SLEEPER_M), track), across, f32(code | (material << 4u)) / 255.0);
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
    if ((job.mode & HEIGHTS) == 0u && job.base != SYNTHESIZED) {
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
