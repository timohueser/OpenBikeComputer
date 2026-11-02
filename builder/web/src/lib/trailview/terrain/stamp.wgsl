// The stamp pass: each page that arrives or is synthesized gets its node's stamps before the mesh,
// drawnHeight and the normals read it. A stored page keeps its raw heights for the syntheses below
// it and draws a stamped copy; a synthesized page reads the raw stored pages only, so no level ever
// upsamples a stamped parent. Near pages get the feature mask.
// #include drawn-height.wgsl
// #include stamp-core.wgsl
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
// Per job and mesh vertex, what the fit reads and writes: its move in units, x and y as 16-bit signed halves, its height there; and, in cm, the most
// that it may stand, the least that it should stand, and the most once it rises.
struct MeshVertex {
    moved: u32,
    raw: f32,
    most: atomic<i32>,
    least: atomic<i32>,
    guard: atomic<i32>,
}
@group(0) @binding(8) var<storage, read_write> mesh_vertices: array<MeshVertex>;
// Per job, the ground that a way holds at each texel of the ring of a mesh cell west and north of its page, from the stamp: a page reads no other.
@group(0) @binding(9) var<storage, read_write> ring: array<f32>;

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
// Where a road's cut or fill meets the ground, the ground rounds over DAYLIGHT_M of height instead of a
// crease. Near the road the slope keeps its crease: the rounding grows from SOFT_FROM_M to SOFT_FULL_M past
// the core.
const DAYLIGHT_M: f32 = 0.8;
const SOFT_FROM_M: f32 = 1.5;
const SOFT_FULL_M: f32 = 4.0;
// How far a triangle may stand over the ground that a way holds, under its face, which lies LIFT_M of bins.ts over that ground; and how far under it, which a way's curtain closes.
const FIT_OVER_M: f32 = 0.02;
const FIT_UNDER_M: f32 = 0.25;
const FIT_PAST_TEXELS: f32 = 0.75;
// A triangle with its three vertices inside a way spans this much at most, by the steepest grade over a cell; more is two ways at a conflict.
const FIT_INSIDE_M: f32 = 0.5;
// No way's sidewalk or shoulder reaches farther past its carriageway: SIDEWALK_M and SHOULDER_M of bins.ts.
const WAY_MOST_M: f32 = 3.0;
// A cell's triangles reach a way's edge from within this many texels of its middle: half its diagonal, with a snap.
const FIT_REACH_TEXELS: f32 = 7.0;
// The coarsest level that a corridor draws on, within RANGE_M of roads.ts.
const FIT_LEVEL: u32 = 17u;
const RING_TEXELS: u32 = 2064u;
const RING_COLUMNS: i32 = 1040;
const NO_MOST: i32 = 2147483647;
const NO_LEAST: i32 = -2147483647;
// The most that a vertex's height shifts, in cm: the 14 signed bits of its word. No road's cut stands higher within a cell.
const SHIFT_MOST: i32 = 8191;

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
        h = select(NONE, clamp(rounded(base, s), s.lo, s.hi), known(base));
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
        let flags = select(0u, WALL_FLAG, faced) | select(0u, CONFLICT_FLAG, s.conflict) | select(0u, FORD_FLAG, s.ford && s.road < 0.0) | select(0u, TAKEN_FLAG, s.taken);
        let surface = select(0u, s.surface, s.road < MASK_RANGE);
        let across = clamp(s.across / (2.0 * select(MASK_RANGE, TRACK_RANGE, surface >= TRACK_CODE)) + 0.5, 0.0, 1.0);
        let walk = f32(walk_byte(s.walk, s.shoulder, s.trodden, s.marked)) / 255.0;
        textureStore(mask_out, in_layer(job.mask, t), job.mask >> 6u,
            vec4f(distance_code(s.road), select(walk, fract(s.sleepers), surface >= TRACK_CODE), across, f32(surface | (flags << 4u)) / 255.0));
    }
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
        shoulder = max(shoulder, select(select(0.0, shoulder_of(surface), holds_shoulder(wide)), TRACK_SPILL_M, surface >= TRACK_CODE));
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
    let word = (bitcast<u32>(moved.x) & 0xffffu) | (bitcast<u32>(moved.y) << 16u);
    let slot = vertex_index(id.z, v);
    mesh_vertices[slot].moved = word;
    mesh_vertices[slot].raw = page_ground(job, vec2f(4 * v) + moved_texels(word, job.level));
    atomicStore(&mesh_vertices[slot].most, NO_MOST);
    atomicStore(&mesh_vertices[slot].least, NO_LEAST);
    atomicStore(&mesh_vertices[slot].guard, NO_MOST);
}

fn vertex_index(job: u32, v: vec2i) -> u32 {
    return job * 4096u + u32(v.y) * 64u + u32(v.x);
}

fn moved_texels(word: u32, level: u32) -> vec2f {
    return vec2f(f32(bitcast<i32>(word << 16u) >> 16u), f32(bitcast<i32>(word) >> 16u)) / f32(1 << (22u - level));
}

// The height of the page's ground at a place in texels past the job's first; NONE where a texel is unknown.
fn page_ground(job: StampJob, at: vec2f) -> f32 {
    let t = at - 0.5;
    let whole = floor(t);
    let part = t - whole;
    let low = job.first + vec2i(whole);
    let a = page_height(job, low);
    let b = page_height(job, low + vec2i(1, 0));
    let c = page_height(job, low + vec2i(0, 1));
    let d = page_height(job, low + vec2i(1, 1));
    if (!(known(a) && known(b) && known(c) && known(d))) {
        return NONE;
    }
    return mix(mix(a, b, part.x), mix(c, d, part.x), part.y);
}

// The outer edge of a way past its carriageway at a mask texel: its sidewalk, or its shoulder.
fn outer_edge(mask: vec4f) -> f32 {
    let wide = walk_code(mask);
    let surface = u32(round(mask.w * 255.0)) & 15u;
    return select(select(select(0.0, shoulder_of(surface), holds_shoulder(wide)), walk_metres(wide), walk_metres(wide) > 0.0), TRACK_SPILL_M, surface >= TRACK_CODE);
}

// The ring: columns -4 to -1 of rows -4 to 259, then rows -4 to -1 of columns 0 to 255.
fn ring_index(t: vec2i) -> u32 {
    let i = select(RING_COLUMNS + (t.y + 4) + 4 * t.x, (t.x + 4) + 4 * (t.y + 4), t.x < 0);
    return u32(clamp(i, 0, i32(RING_TEXELS) - 1));
}

fn ring_texel(i: i32) -> vec2i {
    return select(vec2i((i - RING_COLUMNS) / 4, (i - RING_COLUMNS) % 4 - 4), vec2i(i % 4 - 4, i / 4 - 4), i < RING_COLUMNS);
}

// The stamp at each texel of the ring, ahead of the walks: its top, where a way holds the texel.
@compute @workgroup_size(64)
fn ring_ground(@builtin(global_invocation_id) id: vec3u) {
    let job = stamp_jobs[id.y];
    if (id.x >= RING_TEXELS || job.level < FIT_LEVEL || (job.mode & MASK) == 0u) {
        return;
    }
    let s = stamp_at(job.level, job.first, job.data, ((job.first + ring_texel(i32(id.x))) << vec2u(22u - job.level)) + vec2i(1 << (21u - job.level)));
    ring[id.y * RING_TEXELS + id.x] = select(INF, s.hi, s.road < select(select(select(0.0, shoulder_of(s.surface), s.shoulder), s.walk, s.walk > 0.0), TRACK_SPILL_M, s.surface >= TRACK_CODE));
}

// The ground that a way holds at a texel, `t` past the job's first; INF where none holds it. It counts `past` metres past the edge: the triangle rises from
// the last texel inside, and the face goes on to its edge.
fn way_ground(job: StampJob, index: u32, t: vec2i, past: f32, far_code: f32) -> f32 {
    if (any(t < vec2i(0))) {
        return ring[index * RING_TEXELS + ring_index(t)];
    }
    let texel = job.first + t;
    let mask = textureLoad(mask_in, in_layer(job.mask, texel), job.mask >> 6u, 0);
    // Most texels lie past any way's edge: one compare keeps them off the sinh of the distance's code.
    if (mask.x >= far_code || (u32(round(mask.w * 255.0)) & 15u) == 0u || code_distance(mask.x) >= outer_edge(mask) + past) {
        return INF;
    }
    let h = page_height(job, texel);
    return select(INF, h, known(h));
}

// The most that a vertex at height hp may stand, weight wp at a texel of a triangle whose other two are at hq and hr, so that the
// triangle stands at most FIT_OVER_M over the ground g that a way holds there. The vertices over g share one limit, and those under
// it lend what they lie under it, so the bound holds in one pass and in any order. A vertex that barely reaches a way barely moves.
fn over_limit(g: f32, hp: f32, hq: f32, hr: f32, wp: f32, wq: f32, wr: f32) -> f32 {
    if (hp <= g || wp * hp + wq * hq + wr * hr <= g + FIT_OVER_M) {
        return INF;
    }
    return g + (FIT_OVER_M + max(g - hq, 0.0) * wq + max(g - hr, 0.0) * wr) / (wp + select(0.0, wq, hq > g) + select(0.0, wr, hr > g));
}

// The least, as over_limit, that keeps the triangle at most FIT_UNDER_M under that ground.
fn under_limit(g: f32, hp: f32, hq: f32, hr: f32, wp: f32, wq: f32, wr: f32) -> f32 {
    if (hp >= g || wp * hp + wq * hq + wr * hr >= g - FIT_UNDER_M) {
        return -INF;
    }
    return g - (FIT_UNDER_M + max(hq - g, 0.0) * wq + max(hr - g, 0.0) * wr) / (wp + select(0.0, wq, hq < g) + select(0.0, wr, hr < g));
}

// Whether a place in texels past the job's first lies well inside a way's core.
fn inside_way(job: StampJob, at: vec2f, past: f32) -> bool {
    let mask = textureLoad(mask_in, in_layer(job.mask, job.first + clamp(vec2i(floor(at)), vec2i(0), vec2i(255))), job.mask >> 6u, 0);
    return (u32(round(mask.w * 255.0)) & 15u) != 0u && code_distance(mask.x) < outer_edge(mask) - past;
}

// A triangle of the mesh that the fit walks: its vertices' places in texels past the job's first, and their heights, NONE for a vertex
// off the page, which stays on its grid point within a cell of its move and counts as level with each texel.
struct FitTriangle {
    id: array<vec2i, 3>,
    at: array<vec2f, 3>,
    h: array<f32, 3>,
}

fn on_page(v: vec2i) -> bool {
    return all(v >= vec2i(0)) && all(v <= vec2i(63));
}

fn fit_triangle(job: StampJob, index: u32, id: array<vec2i, 3>, raising: bool) -> FitTriangle {
    var t: FitTriangle;
    t.id = id;
    for (var k = 0; k < 3; k++) {
        t.at[k] = vec2f(4 * id[k]);
        t.h[k] = NONE;
        if (on_page(id[k])) {
            let at = vertex_index(index, id[k]);
            t.at[k] += moved_texels(mesh_vertices[at].moved, job.level);
            let raw = mesh_vertices[at].raw;
            t.h[k] = select(raw, min(raw, f32(atomicLoad(&mesh_vertices[at].most)) * 0.01), raising && known(raw));
        }
    }
    return t;
}

// Walks a triangle's texels of the ways once, for its three vertices at a time. A triangle level within FIT_OVER_M asks nothing, nor one that has no vertex on the page.
fn walk_triangle(job: StampJob, index: u32, id: array<vec2i, 3>, raising: bool) {
    let t = fit_triangle(job, index, id, raising);
    let top = max(t.h[0], max(t.h[1], t.h[2]));
    let least = min(t.h[0], min(t.h[1], t.h[2]));
    if (top <= -1e29 || (known(least) && top - least < FIT_OVER_M)) {
        return;
    }
    let metres = unit_metres(job.first.y << (22u - job.level)) * f32(1 << (22u - job.level));
    let past = FIT_PAST_TEXELS * metres;
    let far_code = distance_code(WAY_MOST_M + past);
    // A triangle inside a way's core lies on its plane, up to a grade: its texels ask nothing, and most of a town's roads are that.
    if (known(least) && top - least < FIT_INSIDE_M && inside_way(job, t.at[0], past) && inside_way(job, t.at[1], past) && inside_way(job, t.at[2], past)) {
        return;
    }
    let ab = t.at[1] - t.at[0];
    let ac = t.at[2] - t.at[0];
    let area = ab.x * ac.y - ab.y * ac.x;
    if (abs(area) < 1e-3) {
        return;
    }
    var most = vec3f(INF);
    var floor_of = vec3f(-INF);
    var guard = vec3f(INF);
    let low = vec2i(ceil(min(t.at[0], min(t.at[1], t.at[2])) - 0.5));
    let high = vec2i(floor(max(t.at[0], max(t.at[1], t.at[2])) - 0.5));
    for (var y = low.y; y <= high.y; y++) {
        for (var x = low.x; x <= high.x; x++) {
            let c = vec2f(f32(x), f32(y)) + 0.5 - t.at[0];
            let w1 = (c.x * ac.y - c.y * ac.x) / area;
            let w2 = (ab.x * c.y - ab.y * c.x) / area;
            let w = vec3f(1.0 - w1 - w2, w1, w2);
            if (min(w.x, min(w.y, w.z)) < 0.0) {
                continue;
            }
            let g = way_ground(job, index, vec2i(x, y), past, far_code);
            if (g >= INF) {
                continue;
            }
            let h = vec3f(select(g, t.h[0], known(t.h[0])), select(g, t.h[1], known(t.h[1])), select(g, t.h[2], known(t.h[2])));
            for (var k = 0; k < 3; k++) {
                if (w[k] <= 0.0) {
                    continue;
                }
                let q = (k + 1) % 3;
                let r = (k + 2) % 3;
                if (raising) {
                    floor_of[k] = max(floor_of[k], under_limit(g, h[k], h[q], h[r], w[k], w[q], w[r]));
                    // All three may rise for other texels: each keeps a third of what is left under the bound here.
                    guard[k] = min(guard[k], h[k] + max(g + FIT_OVER_M - dot(w, h), 0.0) / (3.0 * w[k]));
                } else {
                    most[k] = min(most[k], over_limit(g, h[k], h[q], h[r], w[k], w[q], w[r]));
                }
            }
        }
    }
    for (var k = 0; k < 3; k++) {
        if (!on_page(id[k]) || !known(t.h[k])) {
            continue;
        }
        let at = vertex_index(index, id[k]);
        if (raising) {
            if (floor_of[k] > -INF) {
                atomicMax(&mesh_vertices[at].least, i32(floor(floor_of[k] * 100.0)));
            }
            if (guard[k] < INF) {
                atomicMin(&mesh_vertices[at].guard, i32(ceil(guard[k] * 100.0)));
            }
        } else if (most[k] < INF) {
            atomicMin(&mesh_vertices[at].most, i32(ceil(most[k] * 100.0)));
        }
    }
}

// The two triangles of a cell, from row and column -1, whose ground the neighbouring page holds, so that a vertex on the page's edge hears both its sides. The index
// buffer splits each cell along the diagonal from its upper right to its lower left. Only a level that a corridor draws on fits, and one read keeps the walk off the rest of the page.
fn walk_cell(job: StampJob, index: u32, grid: vec2u, raising: bool) {
    let cell = vec2i(grid) - 1;
    if (job.level < FIT_LEVEL || (job.mode & MASK) == 0u || any(cell > vec2i(63))) {
        return;
    }
    let texel = job.first + clamp(4 * cell + 2, vec2i(0), vec2i(255));
    let mask = textureLoad(mask_in, in_layer(job.mask, texel), job.mask >> 6u, 0);
    let metres = unit_metres(texel.y << (22u - job.level)) * f32(1 << (22u - job.level));
    if ((u32(round(mask.w * 255.0)) & 15u) == 0u || code_distance(mask.x) - outer_edge(mask) > FIT_REACH_TEXELS * metres) {
        return;
    }
    walk_triangle(job, index, array<vec2i, 3>(cell, cell + vec2i(0, 1), cell + vec2i(1, 0)), raising);
    walk_triangle(job, index, array<vec2i, 3>(cell + vec2i(1, 0), cell + vec2i(0, 1), cell + vec2i(1, 1)), raising);
}

// The mesh cannot hold every edge of a way: a snap fails at a ridge of the distance, at a node's edge, in a tight bend and on a way
// narrower than a cell, and a triangle then rises through the way. So first each vertex gets the most that it may stand so that no triangle does.
@compute @workgroup_size(8, 8)
fn lower(@builtin(global_invocation_id) id: vec3u) {
    walk_cell(stamp_jobs[id.z], id.z, id.xy, false);
}

// Then, from the lowered heights, the least that it should stand where the triangles now fall far under a way.
@compute @workgroup_size(8, 8)
fn raise(@builtin(global_invocation_id) id: vec3u) {
    walk_cell(stamp_jobs[id.z], id.z, id.xy, true);
}

// A vertex in water or under a house rises for no way: the mesh does not rise through a shore. Each vertex's move and the shift of its height
// go to the word that the mesh reads, as `snap` of drawn-mesh.wgsl unpacks them.
@compute @workgroup_size(8, 8)
fn fit(@builtin(global_invocation_id) id: vec3u) {
    let job = stamp_jobs[id.z];
    if ((job.mode & MASK) == 0u) {
        return;
    }
    let v = vec2i(id.xy);
    let at = vertex_index(id.z, v);
    let raw = mesh_vertices[at].raw;
    var h = min(raw, f32(atomicLoad(&mesh_vertices[at].most)) * 0.01);
    let flags = u32(round(textureLoad(mask_in, in_layer(job.mask, job.first + clamp(4 * v, vec2i(0), vec2i(255))), job.mask >> 6u, 0).w * 255.0)) >> 4u;
    if ((flags & TAKEN_FLAG) == 0u) {
        h = min(max(h, f32(atomicLoad(&mesh_vertices[at].least)) * 0.01), f32(atomicLoad(&mesh_vertices[at].guard)) * 0.01);
    }
    let shift = select(0, clamp(i32(round((h - raw) * 100.0)), -SHIFT_MOST, SHIFT_MOST), known(raw));
    let moved = mesh_vertices[at].moved;
    textureStore(snaps, v, job.mask, vec4u((moved & 0x1ffu) | (((moved >> 16u) & 0x1ffu) << 9u) | (bitcast<u32>(shift) << 18u)));
}
