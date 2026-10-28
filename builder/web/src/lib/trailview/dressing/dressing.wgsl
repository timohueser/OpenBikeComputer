// The dressing of the ground: herds and rocks placed by the ground's material on the GPU each frame,
// with no stored position. A place comes from hashes of whole world cells, so an animal or a rock
// stands at the same place at every distance and on every load.
//
// Herds: one workgroup per herd cell of HERD_SHIFT. A cell holds the groups of each herd of the region
// at its middle, as many per area as the pack's groups per eye's circle of 400 m; a group stands where
// its middle's material is one that the herd grazes, its animals about it on such material too.
// Rocks: one thread per point of the grid of rocks.wgsl.
// #include ../terrain/drawn-mesh.wgsl
// #include rocks.wgsl
// #include ../ground/look.wgsl
// #include ../instances/emit.wgsl

struct Dressing {
    // The window of herd cells and of rock points: the first, in cells of the world, and a side.
    herd_origin: vec2i,
    herd_side: u32,
    rock_side: u32,
    rock_origin: vec2i,
    // The first of the rock models: SLABS slabs, then as many boulders.
    rock_model: u32,
    unused: u32,
    // Herds stand within this many metres.
    herd_reach: f32,
    unused2: f32,
    unused3: vec2f,
}

// A herd of a region, as dressing.ts writes it: its first model and coats, its groups per eye's circle,
// its breed's height, its animals per group, and the materials that it grazes as bits of their codes.
struct Herd {
    model: u32,
    coats: u32,
    groups: f32,
    height: f32,
    size: vec2f,
    grazes: vec2u,
}

@group(0) @binding(8) var<uniform> dressing: Dressing;
@group(0) @binding(9) var<storage, read> herds: array<Herd>;

// Units along a herd cell edge: about 420 m at 47° N.
const HERD_SHIFT: u32 = 14u;
// Herds per region, groups of a herd per cell at most, and the area of an eye's circle of 400 m.
const HERDS: u32 = 4u;
const GROUPS: u32 = 3u;
const CIRCLE_M2: f32 = 502655.0;
// A group of 8 spreads over SPREAD_M; nothing stands within CLEAR_M of the eye.
const SPREAD_M: f32 = 14.0;
const CLEAR_M: f32 = 6.0;
// Rocks: their kinds.
const SLABS: u32 = 3u;
// Things keep this far off a road, a path or a track.
const ROAD_CLEAR_M: f32 = 1.0;

var<workgroup> cell_region: u32;

@compute @workgroup_size(64)
fn herds_of(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let cell = dressing.herd_origin + vec2i(group.xy);
    let size = 1 << HERD_SHIFT;
    let corner = cell << vec2u(HERD_SHIFT);
    if (lane == 0u) {
        cell_region = finest_cell(lod_space(vec2f(corner - surface.anchor) + 0.5 * f32(size))).b;
    }
    let region = workgroupUniformLoad(&cell_region);
    if (region == NO_CELL.b) {
        return;
    }
    let area = f32(size) * f32(size) * surface.unit * surface.unit;
    for (var k = 0u; k < HERDS; k++) {
        let herd = herds[region * HERDS + k];
        if (herd.groups <= 0.0) {
            break;
        }
        let expected = herd.groups * area / CIRCLE_M2;
        for (var g = 0u; g < GROUPS; g++) {
            let h = mix_bits3(cell.x, cell.y, 16u * k + g);
            if (unit_float(h) >= expected - f32(g)) {
                break;
            }
            let middle = corner + vec2i(vec2f(unit_float(mix_bits(h + 1u)), unit_float(mix_bits(h + 2u))) * f32(size));
            let centre = lod_space(vec2f(middle - surface.anchor));
            if (length(centre) > dressing.herd_reach || !holds(herd.grazes, finest_cell(centre).r)) {
                continue;
            }
            let count = u32(herd.size.x + unit_float(mix_bits(h + 3u)) * (herd.size.y - herd.size.x + 1.0));
            let spread = SPREAD_M * sqrt(max(f32(count), 8.0) / 8.0) / surface.unit;
            for (var i = lane; i < count; i += 64u) {
                let a = mix_bits(h ^ (i * 2654435761u));
                let at = middle + vec2i((vec2f(unit_float(mix_bits(a + 5u)), unit_float(mix_bits(a + 6u))) - 0.5) * spread);
                let s = lod_space(vec2f(at - surface.anchor));
                if (length(s) < CLEAR_M || !holds(herd.grazes, finest_cell(s).r) || kept_off(s, ROAD_CLEAR_M)) {
                    continue;
                }
                let ground = stand_height(s, at);
                if (!known(ground)) {
                    continue;
                }
                // Most of a group wears the breed's first coat.
                let coat = select(1u + mix_bits(a + 7u) % max(herd.coats - 1u, 1u), 0u, unit_float(mix_bits(a + 8u)) < 0.6 || herd.coats == 1u);
                emit(herd.model + coat, s, ground, herd.height * (0.85 + 0.3 * unit_float(mix_bits(a + 9u))),
                    6.2831853 * unit_float(mix_bits(a + 10u)), 1.0, 0.92 + 0.16 * unit_float(mix_bits(a + 11u)), 1.0);
            }
        }
    }
}

@compute @workgroup_size(8, 8)
fn rocks_of(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(dressing.rock_side))) {
        return;
    }
    let point = dressing.rock_origin + vec2i(id.xy);
    let h = rock_hash(point);
    let at = rock_at(point, h);
    let s = lod_space(vec2f(at - surface.anchor));
    let d = length(s);
    if (d > ROCK_REACH_M || d < CLEAR_M) {
        return;
    }
    // The cheap tests first: the view on the level's ground, then the ground's rules, then the slope.
    let coarse = units_height(at, u32(ceil(lod(s))));
    if (!known(coarse) || !visible(render_at(s, coarse), 2.0 * ROCK_HEIGHT_M.y)) {
        return;
    }
    let cell = cell_at(s, u32(ceil(lod(s))));
    let pick = rock_pick(point, h);
    if (pick > rock_density(cell, -1.0)) {
        return;
    }
    let n = drawn_normal(s);
    let slope = sqrt(max(1.0 - n.z * n.z, 0.0)) / max(n.z, 0.1);
    if (pick > rock_density(cell, slope) || kept_off(s, ROAD_CLEAR_M)) {
        return;
    }
    let ground = stand_height(s, at);
    if (!known(ground)) {
        return;
    }
    // The distance and the end of the instances' tier (tier_end of emit.wgsl), the same in the eye's
    // view and the sun's. A rock under half a pixel tall shows nothing, yet takes a slot of the pool and
    // shimmers.
    let far = length(render_at(s, ground));
    let tall = rock_height(h);
    if (far > rock_end(ROCK_REACH_M * params.detail, h) || tall < 0.5 * frame.pixel * far) {
        return;
    }
    let shape = select(SLABS + mix_bits(h + 4u) % SLABS, mix_bits(h + 5u) % SLABS, rock_slab(h));
    emit(dressing.rock_model + shape, s, ground, tall, rock_yaw(h), 1.0, 0.85 + 0.3 * unit_float(mix_bits(h + 9u)), 1.0);
}
