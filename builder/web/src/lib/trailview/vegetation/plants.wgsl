// Ground plants and shrubs about the eye, placed on the GPU each frame from the ground and the packs,
// as the prototype's nearfield.ts placed them: ground plants on a jittered grid of about 0.8 m within
// GROUND_M, all of them within FULL_M and a share of them farther, larger; shrubs on a jittered grid of
// about 6.5 m within SHRUB_M on shrub land, and a third as many on bare land. The grids are world
// cells, so a plant stays where it stands as the eye moves.
// #include ../terrain/drawn-mesh.wgsl
// #include ../ground/look.wgsl
// #include ../instances/emit.wgsl

struct Plants {
    // Per grid the first world cell of its window and its cells a side: the ground plants, the shrubs, and
    // the blades with the model of their patch in w (NO_PLANT for none).
    ground: vec4i,
    shrubs: vec4i,
    blades: vec4i,
    // x: the plant that the blades stand for within BLADES_M, or NO_PLANT.
    sward: vec4u,
}

// Per material code (4 to a vec4) its cover, as COVER_ORDER of ground/rules.ts, and MOOR at bit 8.
// Then per region REGION_WORDS vec4: its flags (MOOR_REGION), its ground plants, its shrubs.
// A ground plant: its model; the covers that it grows on as bits, HABITAT_MOOR, HABITAT_LUSH, CLUMPED;
// its share (bits of an f32); its heights in metres (f16 pair). A shrub: its model; its elevation band
// (f16 pair); its weight (bits of an f32); its heights (f16 pair). A model of 0xffff ends a list.
struct Data {
    covers: array<vec4u, 16>,
    regions: array<vec4u>,
}

@group(0) @binding(8) var<storage, read> data: Data;
@group(0) @binding(9) var<uniform> plants: Plants;

// Units along a cell of the ground plants' grid and of the shrubs' grid: 0.8 m and 6.5 m at 47° N.
const GROUND_UNITS: i32 = 32;
const SHRUB_UNITS: i32 = 256;
const BLADE_UNITS: i32 = 13;
const GROUND_M: f32 = 60.0;
const BLADES_M: f32 = 30.0;
const COVER_CROP: u32 = 3u;
const FULL_M: f32 = 20.0;
const SHRUB_M: f32 = 500.0;
// The eye stands clear of plants this far, and of shrubs this far.
const GROUND_CLEAR_M: f32 = 1.5;
const SHRUB_CLEAR_M: f32 = 10.0;
// The share of the shrub land's grid points that take a shrub.
const SHRUB_SHARE: f32 = 0.5;
const SLOTS: u32 = 8u;
const REGION_WORDS: u32 = 17u;
const MOOR: u32 = 256u;
const MOOR_REGION: u32 = 1u;
const HABITAT_MOOR: u32 = 1u << 12u;
const HABITAT_LUSH: u32 = 1u << 13u;
const CLUMPED: u32 = 1u << 14u;
const NO_PLANT: u32 = 0xffffu;

fn plant_bits(a: u32) -> u32 {
    var x = a * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn plant_hash(cell: vec2i, salt: u32) -> f32 {
    return f32(plant_bits(u32(cell.x) ^ plant_bits(u32(cell.y) ^ plant_bits(salt))) >> 8u) / 16777216.0;
}

fn cover_of(material: u32) -> u32 {
    return data.covers[material / 4u][material % 4u];
}

// Wavy patches 0 to 1, a few tens of metres across, other ones per salt: the prototype's patchAt.
fn stand_of(u: vec2i, salt: f32) -> f32 {
    let p = vec2f(u & vec2i(0xfffff)) * surface.unit;
    return 0.5 + 0.25 * (sin(p.x / 13.0 + salt) * sin(p.y / 17.0 - 1.7 * salt) + sin((p.x + p.y) / 23.0 + 2.3 * salt) * sin((p.x - p.y) / 19.0 + salt));
}

// Whether a point may show, before its ground is known: about the ground under the eye, within a
// slope of 35° from it.
fn roughly_visible(s: vec2f, d: f32) -> bool {
    return visible(render_at(s, frame.eye - surface.above), 0.7 * d + 10.0);
}

// The point of a grid cell: its corner and a jitter of up to the cell.
fn grid_point(cell: vec2i, units: i32) -> vec2i {
    return cell * units + vec2i(vec2f(plant_hash(cell, 1u), plant_hash(cell, 2u)) * f32(units));
}

@compute @workgroup_size(8, 8)
fn ground_plants(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(u32(plants.ground.z)))) {
        return;
    }
    let cell = plants.ground.xy + vec2i(id.xy);
    let u = grid_point(cell, GROUND_UNITS);
    let s = lod_space(vec2f(u - surface.anchor));
    let d = length(s);
    if (d > GROUND_M || d < GROUND_CLEAR_M) {
        return;
    }
    if (!roughly_visible(s, d)) {
        return;
    }
    // Past FULL_M a share of the plants stands, a little larger: from afar the count shows less than the cost.
    let keep = select(max(0.3, FULL_M / d), 1.0, d < FULL_M);
    if (plant_hash(cell, 3u) > keep) {
        return;
    }
    let ground = cell_at(s, u32(ceil(lod(s))));
    if ((ground.z + 1u) * REGION_WORDS > arrayLength(&data.regions)) {
        return;
    }
    let cover = cover_of(ground.x);
    let region = ground.z * REGION_WORDS;
    let moor = (data.regions[region].x & MOOR_REGION) != 0u;
    // The rarer plants come first in the list, so the common one does not take every spot.
    var pick = NO_PLANT;
    var plant = vec4u(0u);
    for (var k = 0u; k < SLOTS; k++) {
        let p = data.regions[region + 1u + k];
        if ((p.x & 0xffffu) == NO_PLANT) {
            break;
        }
        let on = (p.y & (1u << (cover & 255u))) != 0u;
        // In a region with moor, a moor plant also grows on its heath and bog, and a lush one only off them.
        let heath = (cover & MOOR) != 0u;
        var grows = on;
        if (moor && (p.y & HABITAT_MOOR) != 0u) {
            grows = heath && (on || (cover & 255u) == COVER_GRASS);
        } else if (moor && (p.y & HABITAT_LUSH) != 0u) {
            grows = !heath && on;
        }
        let share = bitcast<f32>(p.z) * select(1.0, 2.5 * smoothstep(0.55, 0.7, stand_of(u, f32(k))), (p.y & CLUMPED) != 0u);
        if (grows && plant_hash(cell, 10u + k) < share) {
            pick = p.x & 0xffffu;
            plant = p;
            break;
        }
    }
    // Snow covers the share of the ground plants that it lies on.
    if (pick == NO_PLANT || (pick == plants.sward.x && d < BLADES_M) || on_road(s, 0.3) || plant_hash(cell, 7u) < snow_at(s)) {
        return;
    }
    let h = stand_height(s, u);
    if (!known(h)) {
        return;
    }
    let heights = unpack2x16float(plant.w);
    let height = mix(heights.x, heights.y, plant_hash(cell, 4u)) / sqrt(sqrt(keep));
    emit(pick, render_at(s, h), height, 6.2831853 * plant_hash(cell, 5u), 1.0, 0.88 + 0.24 * plant_hash(cell, 6u), 1.0);
}

@compute @workgroup_size(8, 8)
fn shrubs(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(u32(plants.shrubs.z)))) {
        return;
    }
    let cell = plants.shrubs.xy + vec2i(id.xy);
    let u = grid_point(cell, SHRUB_UNITS);
    let s = lod_space(vec2f(u - surface.anchor));
    let d = length(s);
    if (d > SHRUB_M || d < SHRUB_CLEAR_M) {
        return;
    }
    if (!roughly_visible(s, d)) {
        return;
    }
    let ground = cell_at(s, u32(ceil(lod(s))));
    if ((ground.z + 1u) * REGION_WORDS > arrayLength(&data.regions)) {
        return;
    }
    let cover = cover_of(ground.x) & 255u;
    // Bare ground is mostly "bare or sparse vegetation": scattered shrubs, a third as dense.
    let share = select(select(0.0, SHRUB_SHARE / 3.0, cover == COVER_BARE), SHRUB_SHARE, cover == COVER_SHRUB);
    if (plant_hash(cell, 3u) > share) {
        return;
    }
    let h = stand_height(s, u);
    if (!known(h)) {
        return;
    }
    // Shrubs keep to their elevation band, where any shrub fits there; then by weight.
    let region = ground.z * REGION_WORDS + 1u + SLOTS;
    var total = vec2f(0.0);
    for (var k = 0u; k < SLOTS; k++) {
        let p = data.regions[region + k];
        if ((p.x & 0xffffu) == NO_PLANT) {
            break;
        }
        let band = unpack2x16float(p.y);
        total += bitcast<f32>(p.z) * vec2f(f32(h >= band.x && h <= band.y), 1.0);
    }
    let banded = total.x > 0.0;
    var left = plant_hash(cell, 7u) * select(total.y, total.x, banded);
    var pick = vec4u(NO_PLANT);
    for (var k = 0u; k < SLOTS; k++) {
        let p = data.regions[region + k];
        if ((p.x & 0xffffu) == NO_PLANT) {
            break;
        }
        let band = unpack2x16float(p.y);
        if (!banded || (h >= band.x && h <= band.y)) {
            pick = p;
            left -= bitcast<f32>(p.z);
            if (left < 0.0) {
                break;
            }
        }
    }
    let model = pick.x & 0xffffu;
    if (model == NO_PLANT) {
        return;
    }
    let heights = unpack2x16float(pick.w);
    let height = mix(heights.x, heights.y, plant_hash(cell, 4u));
    if (on_road(s, models[model].spread * height)) {
        return;
    }
    let yaw = 6.2831853 * plant_hash(cell, 5u);
    emit_tree(model, render_at(s, h), height, yaw, 1.0, 0.88 + 0.24 * plant_hash(cell, 6u), 1.0, 0.0, pick_of(yaw, height), snow_at(s));
}

// The near sward: a patch of blades on a grid of 0.33 m within BLADES_M, on grass and crops off the
// moor, where the blades stand for the plant `sward`.
@compute @workgroup_size(8, 8)
fn blades(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(u32(plants.blades.z)))) {
        return;
    }
    let cell = plants.blades.xy + vec2i(id.xy);
    let u = grid_point(cell, BLADE_UNITS);
    let s = lod_space(vec2f(u - surface.anchor));
    let d = length(s);
    if (d > BLADES_M || d < 0.5 || !roughly_visible(s, d)) {
        return;
    }
    let ground = cell_at(s, u32(ceil(lod(s))));
    let cover = cover_of(ground.x);
    if (((cover & 255u) != COVER_GRASS && (cover & 255u) != COVER_CROP) || (cover & MOOR) != 0u || on_road(s, 0.2) || plant_hash(cell, 7u) < snow_at(s)) {
        return;
    }
    let h = plain_height(s);
    if (!known(h)) {
        return;
    }
    emit(u32(plants.blades.w), render_at(s, h), 0.25 + 0.15 * plant_hash(cell, 4u), 6.2831853 * plant_hash(cell, 5u), 1.0, 0.85 + 0.3 * plant_hash(cell, 6u), 1.0);
}
