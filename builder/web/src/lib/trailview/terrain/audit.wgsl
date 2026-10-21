// The invariant audit: drawnHeight at samples along the features near the eye, checked against what
// the stamps promise at the level that draws each sample, and the stamps of neighbouring nodes at
// the texels on both sides of their border.
// #include features.wgsl
// #include stamp-core.wgsl

struct Sample {
    at: vec2i,
    kind: u32,
    // The road height at `at`, or the lake level.
    expected: f32,
    // The half-width of the road or river, 0 for a lake.
    half: f32,
    // Metres from the shore.
    shore: f32,
    // Metres to the nearest junction, and its widest half-width.
    junction: f32,
    junction_half: f32,
    // The other edge of a cross-fall pair, and the road height there.
    other: vec2i,
    other_expected: f32,
    unused: u32,
}

// Two nodes of a level side by side: B lies east of A (axis 0) or south of it (axis 1).
struct Pair {
    first_a: vec2i,
    first_b: vec2i,
    level: u32,
    data_a: u32,
    data_b: u32,
    axis: u32,
}

@group(0) @binding(0) var<storage, read> samples: array<Sample>;
@group(0) @binding(2) var<storage, read_write> counts: array<atomic<u32>>;
// The first violations: per entry the point in LOD space, drawnHeight, the expected height; then the kind, the level, the shore distance.
@group(0) @binding(3) var<storage, read_write> found: array<vec4f>;
@group(0) @binding(4) var<storage, read> pairs: array<Pair>;
// `stamp_data` starts with a table: per level, MAP × MAP nodes about the eye as the LOD map has
// them, the offset + 1 of each node's stamp data, 0 for none.

override BAKED: bool = false;

const CROSS_FALL: u32 = 1u;
const ROAD_HEIGHT: u32 = 2u;
const IN_WATER: u32 = 3u;
const ON_BANK: u32 = 4u;
const SHORE_SIDE: u32 = 5u;
// counts: per kind 1 to 5 at 6 × kind the checked samples, the violations, the conflicts, the misses
// within the reach of a junction, the largest error elsewhere and the largest at a junction in
// micrometres; 36 apron texels compared, 37 apron texels that differ; 38 the violations found.
const SLOTS: u32 = 6u;
const APRON_CHECKS: u32 = 36u;
const FOUND: u32 = 38u;
const FOUND_MAX: u32 = 64u;
// Cross-fall on a carriageway, as the epic sets it, and a kink of the grade across one mesh cell: the
// mesh draws a straight line between its vertices, 4 texels apart, so a bend or a change of grade of
// the profile between them is lost. A road height may leave its profile by the core tolerance and the
// kink, and a river level changes along it in the same way.
const CROSS_FALL_MAX: f32 = 0.05;
const ROAD_KINK: f32 = 0.03;
// The float32 rounding of a height of some 1000 m, and of the mesh position about the eye.
const ROUNDING: f32 = 0.002;
// As TILE_ZOOM + 1 of pages.ts: baked conditioning stamps the stored levels up to it, and the finer levels upsample them.
const STORED_FINEST: u32 = 14u;

// Counts a checked sample with its error and what it may leave: a miss near a junction, where the
// simple profiles of its roads meet, a conflict, or a violation.
fn check(kind: u32, e: f32, limit: f32, junction: bool, s: vec2f, level: u32, drawn: f32, expected: f32, detail: f32) {
    let at = SLOTS * kind;
    atomicAdd(&counts[at], 1u);
    atomicMax(&counts[at + select(4u, 5u, junction)], u32(clamp(e, 0.0, 4000.0) * 1e6));
    if (e <= limit) {
        return;
    }
    if (junction) {
        atomicAdd(&counts[at + 3u], 1u);
        return;
    }
    if (conflict_near(s, level)) {
        atomicAdd(&counts[at + 2u], 1u);
        return;
    }
    atomicAdd(&counts[at + 1u], 1u);
    let n = atomicAdd(&counts[FOUND], 1u);
    if (n < FOUND_MAX) {
        found[2u * n] = vec4f(s, drawn, expected);
        found[2u * n + 1u] = vec4f(f32(kind), f32(level), detail, 0.0);
    }
}

// The stamp data of the node of a level that holds a world point, from the node table, NO_DATA for none.
fn node_data(at: vec2i, level: u32) -> u32 {
    let slot = (at >> vec2u(30u - level)) - surface.levels[level].xy;
    if (any(slot < vec2i(0)) || any(slot >= vec2i(MAP))) {
        return NO_DATA;
    }
    let data = stamp_data[(i32(level) * MAP + slot.y) * MAP + slot.x];
    return select(NO_DATA, data - 1u, data != 0u);
}

// Whether a texel of a level within the reach of a point's mesh has a conflict in its stamps: there
// the higher bucket won, and the mask flags it. A page with a mask holds the flag; another page stamps the texel again.
fn conflict_near(s: vec2f, level: u32) -> bool {
    let t = texel_of(s, level).whole;
    let reach = i32(MARGIN_TEXELS);
    let shift = 22u - level;
    for (var j = -reach; j <= reach + 1; j++) {
        for (var i = -reach; i <= reach + 1; i++) {
            let whole = t + vec2i(i, j);
            let mask = mask_of(level, whole);
            if (mask != 0u) {
                if ((textureLoad(mask_kinds, in_layer(mask - 1u, whole), (mask - 1u) >> 6u, 0).z & CONFLICT) != 0u) {
                    return true;
                }
                continue;
            }
            let p = (whole << vec2u(shift)) + vec2i(1 << (shift - 1u));
            let data = node_data(p, level);
            if (data != NO_DATA && stamp_at(level, (whole >> vec2u(8u)) << vec2u(8u), data, p, BAKED).conflict) {
                return true;
            }
        }
    }
    return false;
}

// Whether a vertex within two mesh cells of a point morphs toward the coarser level.
fn morphs_near(s: vec2f, level: u32) -> bool {
    let reach = 2.0 * surface.unit * f32(1 << (24u - level));
    for (var i = 0; i < 9; i++) {
        if (lod(s + reach * vec2f(f32(i % 3 - 1), f32(i / 3 - 1))) < f32(level)) {
            return true;
        }
    }
    return false;
}

fn texel_m(at: vec2i, level: u32) -> f32 {
    return unit_metres(at.y) * f32(1 << (22u - level));
}

@compute @workgroup_size(64)
fn audit(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= arrayLength(&samples)) {
        return;
    }
    let q = samples[id.x];
    let s = lod_space(vec2f(q.at - surface.anchor));
    let level = drawn_level(s);
    // In or near the morph band the vertices blend toward the coarser level, so it has to show the feature too.
    let coarse = select(level, level - 1u, morphs_near(s, level));
    let fine = texel_m(q.at, level);
    let wide = texel_m(q.at, coarse);
    let reach = MARGIN_TEXELS * fine;
    let kink = ROAD_KINK * MESH_TEXELS * wide;
    if (q.kind == SHORE_SIDE) {
        // The stamps put a point more than a texel from a shore on its side: in the water or out of it.
        let data = node_data(q.at, level);
        if (abs(q.shore) < fine || data == NO_DATA) {
            return;
        }
        let stamp = stamp_at(level, (q.at >> vec2u(30u - level)) << vec2u(8u), data, q.at, BAKED);
        check(SHORE_SIDE, select(0.0, abs(q.shore), (stamp.shore < 0.0) != (q.shore < 0.0)), 0.0, false, s, level, stamp.shore, q.shore, q.shore);
        return;
    }
    let drawn = drawn_height(s);
    if (!known(drawn)) {
        return;
    }
    if (q.kind == CROSS_FALL || q.kind == ROAD_HEIGHT) {
        // Baked conditioning stamps the stored levels only.
        if (!road_shows(q.half, level, BAKED) || !road_shows(q.half, coarse, BAKED) || (BAKED && level > STORED_FINEST)) {
            return;
        }
        let junction = q.junction < 2.0 * junction_reach(q.junction_half, fine, reach) + q.half;
        if (q.kind == CROSS_FALL) {
            let t = lod_space(vec2f(q.other - surface.anchor));
            let other = drawn_height(t);
            if (!known(other) || drawn_level(t) != level) {
                return;
            }
            check(CROSS_FALL, abs((other - drawn) - (q.other_expected - q.expected)), CROSS_FALL_MAX + kink, junction, 0.5 * (s + t), level, other - drawn, q.other_expected - q.expected, q.junction);
        } else {
            check(ROAD_HEIGHT, abs(drawn - q.expected), CORE_TOL + kink + ROUNDING, junction, s, level, drawn, q.expected, q.junction);
        }
        return;
    }
    // A river shapes only the levels that show it, and its level changes along it.
    if (q.half > 0.0 && (!river_shows(q.half, level) || !river_shows(q.half, coarse))) {
        return;
    }
    let along = select(0.0, kink, q.half > 0.0);
    if (q.kind == IN_WATER && q.shore >= reach) {
        check(IN_WATER, drawn - (q.expected - LAKE_DEPTH), ROUNDING + along, false, s, level, drawn, q.expected, q.shore);
    }
    if (q.kind == ON_BANK && banks_show(level, BAKED) && banks_show(coarse, BAKED) && q.shore >= reach && q.shore <= reach + BANK_M) {
        check(ON_BANK, q.expected + FREEBOARD - drawn, ROUNDING + along, false, s, level, drawn, q.expected, q.shore);
    }
}

// The stamps of the two nodes of a pair at a texel of each side of their border: equal intervals give
// equal heights from any base.
@compute @workgroup_size(64)
fn apron(@builtin(global_invocation_id) id: vec3u) {
    let pair = pairs[id.y];
    let along = i32(id.x % 256u);
    let side = i32(id.x / 256u);
    let shift = 22u - pair.level;
    // The last texel column (row) of A, then the first of B.
    var t = pair.first_b + select(vec2i(side - 1, along), vec2i(along, side - 1), pair.axis == 1u);
    let p = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let a = stamp_at(pair.level, pair.first_a, pair.data_a, p, BAKED);
    let b = stamp_at(pair.level, pair.first_b, pair.data_b, p, BAKED);
    atomicAdd(&counts[APRON_CHECKS], 1u);
    if (bitcast<u32>(a.lo) != bitcast<u32>(b.lo) || bitcast<u32>(a.hi) != bitcast<u32>(b.hi) || a.conflict != b.conflict || bitcast<u32>(a.water) != bitcast<u32>(b.water)) {
        atomicAdd(&counts[APRON_CHECKS + 1u], 1u);
        let n = atomicAdd(&counts[FOUND], 1u);
        if (n < FOUND_MAX) {
            found[2u * n] = vec4f(vec2f(t), a.lo - b.lo, a.hi - b.hi);
            found[2u * n + 1u] = vec4f(6.0, f32(pair.level), a.water - b.water, 0.0);
        }
    }
}
