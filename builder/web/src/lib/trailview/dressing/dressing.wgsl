// The dressing of the ground: herds and rocks placed by the ground's material on the GPU each frame,
// with no stored position. A place comes from hashes of whole world cells, so an animal or a rock
// stands at the same place at every distance and on every load.
//
// Herds: one workgroup per herd cell of HERD_SHIFT. A cell holds the groups of each herd of the region
// at its middle, as many per area as the pack's groups per eye's circle of 400 m; a group stands where
// its middle's material is one that the herd grazes, its animals about it on such material too.
// Rocks: one thread per point of a grid of ROCK_SHIFT, jittered: a rock stands on open or rocky
// ground as the region's outcrops, the slope and the hardness of the rock under it say.
// #include ../terrain/drawn-mesh.wgsl
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
    // Herds stand within x metres, rocks within y.
    reach: vec2f,
    unused2: vec2f,
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

// Units along a herd cell edge: about 420 m at 47° N; and along a rock point's cell: about 3.3 m.
const HERD_SHIFT: u32 = 14u;
const ROCK_SHIFT: u32 = 7u;
// Herds per region, groups of a herd per cell at most, and the area of an eye's circle of 400 m.
const HERDS: u32 = 4u;
const GROUPS: u32 = 3u;
const CIRCLE_M2: f32 = 502655.0;
// A group of 8 spreads over SPREAD_M; nothing stands within CLEAR_M of the eye.
const SPREAD_M: f32 = 14.0;
const CLEAR_M: f32 = 6.0;
// Rocks: their heights, their kinds, and the slope past which no boulder lies.
const ROCK_HEIGHT_M: vec2f = vec2f(0.25, 1.3);
const SLABS: u32 = 3u;
const STEEPEST: f32 = 0.85;
// Things nearer than this stand on drawnHeight, farther ones on the level that draws them.
const MESH_HEIGHT_M: f32 = 400.0;

fn mix_bits(a: u32) -> u32 {
    var x = a * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn mix_bits3(x: i32, y: i32, z: u32) -> u32 {
    return mix_bits(u32(x) ^ mix_bits(u32(y) ^ mix_bits(z)));
}

fn unit_float(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// Whether the feature mask keeps a thing off a point: on a road, a path or a track.
fn on_road(s: vec2f) -> bool {
    let mask = mask_at(s, u32(ceil(lod(s))), true);
    return mask.level != 0u && mask.kind.x != 0u && mask_metres(mask.shape.x) < 1.0;
}

fn stand_height(s: vec2f, u: vec2i) -> f32 {
    if (length(s) < MESH_HEIGHT_M) {
        return plain_height(s);
    }
    return units_height(u, u32(ceil(lod(s))));
}

fn grazes(herd: Herd, material: u32) -> bool {
    return material < 64u && ((herd.grazes[material >> 5u] >> (material & 31u)) & 1u) == 1u;
}

var<workgroup> cell_region: u32;

@compute @workgroup_size(64)
fn herds_of(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let cell = dressing.herd_origin + vec2i(group.xy);
    let size = 1 << HERD_SHIFT;
    let corner = cell << vec2u(HERD_SHIFT);
    if (lane == 0u) {
        cell_region = cell_at(lod_space(vec2f(corner - surface.anchor) + 0.5 * f32(size))).b;
    }
    let region = workgroupUniformLoad(&cell_region);
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
            if (length(centre) > dressing.reach.x || !grazes(herd, cell_at(centre).r)) {
                continue;
            }
            let count = u32(herd.size.x + unit_float(mix_bits(h + 3u)) * (herd.size.y - herd.size.x + 1.0));
            let spread = SPREAD_M * sqrt(max(f32(count), 8.0) / 8.0) / surface.unit;
            for (var i = lane; i < count; i += 64u) {
                let a = mix_bits(h ^ (i * 2654435761u));
                let at = middle + vec2i((vec2f(unit_float(mix_bits(a + 5u)), unit_float(mix_bits(a + 6u))) - 0.5) * spread);
                let s = lod_space(vec2f(at - surface.anchor));
                if (length(s) < CLEAR_M || !grazes(herd, cell_at(s).r) || on_road(s)) {
                    continue;
                }
                let ground = stand_height(s, at);
                if (!known(ground)) {
                    continue;
                }
                // Most of a group wears the breed's first coat.
                let coat = select(1u + mix_bits(a + 7u) % max(herd.coats - 1u, 1u), 0u, unit_float(mix_bits(a + 8u)) < 0.6 || herd.coats == 1u);
                emit(herd.model + coat, render_at(s, ground), herd.height * (0.85 + 0.3 * unit_float(mix_bits(a + 9u))),
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
    let h = mix_bits3(point.x, point.y, 77u);
    let at = (point << vec2u(ROCK_SHIFT)) + vec2i(vec2f(unit_float(mix_bits(h + 1u)), unit_float(mix_bits(h + 2u))) * f32(1 << ROCK_SHIFT));
    let s = lod_space(vec2f(at - surface.anchor));
    let d = length(s);
    if (d > dressing.reach.y || d < CLEAR_M) {
        return;
    }
    let cell = cell_at(s);
    let kinds = rule(cell.b, slot_kinds() + i32(cell.r));
    // Open ground or exposed rock: kinds.a open, kinds.z rocky.
    if (kinds.a == 0.0 && kinds.z == 0.0) {
        return;
    }
    let outcrops = rule(cell.b, slot_cliff() + 3).x;
    let hardness = rule(cell.b, slot_hardness() + i32(cell.g)).x;
    let n = drawn_normal(s);
    let slope = sqrt(max(1.0 - n.z * n.z, 0.0)) / max(n.z, 0.1);
    if (outcrops <= 0.0 || slope > STEEPEST) {
        return;
    }
    // As the prototype's near field: hard rock breaks through even on level ground; mapped rock shows as rocks.
    let mapped = 0.9 * kinds.z * smoothstep(0.0, 0.6, outcrops) * smoothstep(0.2, 0.6, hardness);
    let rocky = max(outcrops * (0.3 + 1.4 * hardness) * clamp(1.4 * slope + 0.6 * (hardness - 0.5) - 0.05, 0.0, 1.0), mapped);
    if (unit_float(mix_bits(h + 3u)) > 0.55 * rocky || on_road(s)) {
        return;
    }
    let ground = stand_height(s, at);
    if (!known(ground)) {
        return;
    }
    let shape = select(SLABS + mix_bits(h + 4u) % SLABS, mix_bits(h + 5u) % SLABS, unit_float(mix_bits(h + 6u)) < 0.7);
    let tall = ROCK_HEIGHT_M.x + pow(unit_float(mix_bits(h + 7u)), 2.0) * (ROCK_HEIGHT_M.y - ROCK_HEIGHT_M.x);
    emit(dressing.rock_model + shape, render_at(s, ground), tall, 6.2831853 * unit_float(mix_bits(h + 8u)), 1.0, 0.85 + 0.3 * unit_float(mix_bits(h + 9u)), 1.0);
}
