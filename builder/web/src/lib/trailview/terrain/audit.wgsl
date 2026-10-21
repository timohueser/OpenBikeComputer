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
    // The road's half-width.
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
const IN_LAKE: u32 = 3u;
const ON_BANK: u32 = 4u;
// counts: 0 water samples on a page without a water layer; per kind 1 to 4 at 4 × kind the checked
// samples, the violations, the conflicts, the largest error in micrometres; 20 water checked, 21 water
// wrong; 22 apron texels compared, 23 apron texels that differ; 24 the violations found.
const WATER_CHECKS: u32 = 20u;
const APRON_CHECKS: u32 = 22u;
const FOUND: u32 = 24u;
const FOUND_MAX: u32 = 64u;
// Cross-fall on a carriageway, as the epic sets it, and a kink of the grade across one mesh cell: the
// mesh draws a straight line between its vertices, 4 texels apart, so a bend or a change of grade of
// the profile between them is lost. A road height may leave its profile by the core tolerance and the kink.
const CROSS_FALL_MAX: f32 = 0.05;
const ROAD_KINK: f32 = 0.03;
// The float32 rounding of a height of some 1000 m, and of the mesh position about the eye.
const ROUNDING: f32 = 0.002;

fn violate(kind: u32, s: vec2f, drawn: f32, expected: f32, level: u32, shore: f32) {
    atomicAdd(&counts[4u * kind + 1u], 1u);
    let n = atomicAdd(&counts[FOUND], 1u);
    if (n < FOUND_MAX) {
        found[2u * n] = vec4f(s, drawn, expected);
        found[2u * n + 1u] = vec4f(f32(kind), f32(level), shore, 0.0);
    }
}

fn error(kind: u32, e: f32) {
    atomicMax(&counts[4u * kind + 3u], u32(clamp(e, 0.0, 4000.0) * 1e6));
}

// Whether a texel of a level within the reach of a point's mesh has a conflict in its stamps: there
// the higher bucket won, and the shader draws a wall.
fn conflict_near(s: vec2f, level: u32) -> bool {
    let t = texel_of(s, level).whole;
    let reach = i32(MARGIN_TEXELS);
    let shift = 22u - level;
    for (var j = -reach; j <= reach + 1; j++) {
        for (var i = -reach; i <= reach + 1; i++) {
            let whole = t + vec2i(i, j);
            let node = whole >> vec2u(8u);
            let slot = node - surface.levels[level].xy;
            if (any(slot < vec2i(0)) || any(slot >= vec2i(MAP))) {
                continue;
            }
            let data = stamp_data[(i32(level) * MAP + slot.y) * MAP + slot.x];
            if (data != 0u && stamp_at(level, node << vec2u(8u), data - 1u, (whole << vec2u(shift)) + vec2i(1 << (shift - 1u)), BAKED).conflict) {
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
    let drawn = drawn_height(s);
    if (!known(drawn)) {
        return;
    }
    if (q.kind == CROSS_FALL || q.kind == ROAD_HEIGHT) {
        if (!road_shows(q.half, fine, BAKED) || !road_shows(q.half, wide, BAKED)) {
            return;
        }
        // Junctions relax the cores near them, up to twice the widest reach of a junction.
        if (q.junction < 2.0 * min(JUNCTION_CAP, reach_limit(fine) * 0.5) + q.half) {
            return;
        }
        var e = 0.0;
        var limit = 0.0;
        if (q.kind == CROSS_FALL) {
            let t = lod_space(vec2f(q.other - surface.anchor));
            let other = drawn_height(t);
            if (!known(other) || drawn_level(t) != level) {
                return;
            }
            if (conflict_near(t, level)) {
                atomicAdd(&counts[4u * q.kind], 1u);
                atomicAdd(&counts[4u * q.kind + 2u], 1u);
                return;
            }
            e = abs((other - drawn) - (q.other_expected - q.expected));
            limit = CROSS_FALL_MAX + ROAD_KINK * MESH_TEXELS * wide;
        } else {
            e = abs(drawn - q.expected);
            limit = CORE_TOL + ROAD_KINK * MESH_TEXELS * wide + ROUNDING;
        }
        atomicAdd(&counts[4u * q.kind], 1u);
        if (e > limit) {
            if (conflict_near(s, level)) {
                atomicAdd(&counts[4u * q.kind + 2u], 1u);
                return;
            }
            violate(q.kind, s, drawn, q.expected, level, e);
        }
        error(q.kind, e);
        return;
    }
    if (q.kind == IN_LAKE) {
        if (q.shore < reach) {
            return;
        }
        atomicAdd(&counts[4u * IN_LAKE], 1u);
        let top = q.expected - LAKE_DEPTH + ROUNDING;
        if (drawn > top) {
            violate(IN_LAKE, s, drawn, q.expected, level, q.shore);
        }
        error(IN_LAKE, drawn - q.expected + LAKE_DEPTH);
        // A page that found no water layer in its pool cannot hold the level.
        if (extras_of(level, texel_of(s, level).whole).y == 0u) {
            atomicAdd(&counts[0], 1u);
            return;
        }
        atomicAdd(&counts[WATER_CHECKS], 1u);
        let water = water_at(s, level);
        if (water != q.expected) {
            atomicAdd(&counts[WATER_CHECKS + 1u], 1u);
            let n = atomicAdd(&counts[FOUND], 1u);
            if (n < FOUND_MAX) {
                found[2u * n] = vec4f(s, water, q.expected);
                found[2u * n + 1u] = vec4f(5.0, f32(level), q.shore, 0.0);
            }
        }
        return;
    }
    if (q.kind == ON_BANK) {
        if (!banks_show(fine, BAKED) || !banks_show(wide, BAKED) || q.shore < reach || q.shore > reach + BANK_M) {
            return;
        }
        atomicAdd(&counts[4u * ON_BANK], 1u);
        let bottom = q.expected + FREEBOARD - ROUNDING;
        if (drawn < bottom) {
            if (conflict_near(s, level)) {
                atomicAdd(&counts[4u * ON_BANK + 2u], 1u);
                return;
            }
            violate(ON_BANK, s, drawn, q.expected, level, q.shore);
        }
        error(ON_BANK, q.expected + FREEBOARD - drawn);
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
