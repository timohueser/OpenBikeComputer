// The forest: trees placed from the canopy layer on the GPU each frame, with no stored position.
//
// The world is cut into base cells of CELL units. Each holds the same progressive blue-noise points,
// moved by a hash of the cell; point r of a base cell is a tree where r < the trees per base cell
// that the canopy at the point gives. So a tree stands at the same place at every distance, and a
// tree that stands far away stands near too. Past `thin` metres the trees thin with the distance:
// the first points of a cell stay, and they stand wider.
//
// Far away few trees stand per base cell, so `find_cells` groups them: a cell of level k is 2^k × 2^k
// base cells, and its rank R is point R >> 2k of its base cell R & (4^k − 1). Its trees are those of
// its base cells, so a change of level moves no tree. A cell is of the coarsest level whose nearest
// point lies `thin` × 2^k metres away or more. `trees` runs one workgroup per cell and emits its trees
// on the drawn surface.
// #include ../terrain/drawn-mesh.wgsl
// #include ../ground/look.wgsl
// #include ../instances/emit.wgsl

struct Forest {
    // The window of base cells: its first cell, in base cells of the world, and its cells a side.
    origin: vec2i,
    side: u32,
    // Points per base cell.
    points: u32,
    // Trees stand within `reach` metres; past `thin` metres they thin.
    reach: f32,
    thin: f32,
    // Trees per square metre of each stems class (specs/trailview-tiles.md#canopy).
    stems: array<vec4f, 2>,
}

// What the forest reads, in one buffer: the canopy's window, the points of a base cell, the species.
struct Data {
    // The canopy window's first tile, and per tile of the window its layer + 1, or 0.
    origin: vec2i,
    unused: vec2u,
    tiles: array<u32, 64>,
    // Progressive blue noise in the unit square.
    points: array<vec2f, 512>,
    // Per region SPECIES species: the model and leaf type (1 broadleaved, 2 needleleaved) as bits of an
    // f32, the elevation band in metres and the weight; weight 0 ends a region's list.
    species: array<vec4f>,
}

// A cell of the list: its corner in base cells, its trees at most in the low 16 bits and its level in
// the high bits, and the height of its middle (as bits of an f32).
struct CellList {
    // The workgroups of `trees`: one per cell of the list.
    groups: atomic<u32>,
    y: u32,
    z: u32,
    unused: u32,
    list: array<vec4i>,
}

// The cell list as `trees` reads it.
struct Listed {
    header: vec4u,
    list: array<vec4i>,
}

@group(0) @binding(8) var canopy: texture_2d_array<u32>;
@group(0) @binding(9) var<storage, read> data: Data;
// The cells that `find_cells` finds, which `trees` reads; its count is the indirect dispatch of `trees`.
@group(0) @binding(10) var<storage, read_write> found: CellList;
@group(0) @binding(11) var<uniform> forest: Forest;
@group(0) @binding(12) var<storage, read> listed: Listed;

// Units along a base cell edge: the nodes of level 19, 52 m at 47° N.
const CELL_SHIFT: u32 = 11u;
const CELL: i32 = 2048;
// The coarsest level of a cell: 8 × 8 base cells.
const LEVELS: u32 = 3u;
// Units along a canopy cell edge: zoom 12 at 256 cells, 26 m at 47° N.
const CANOPY_SHIFT: u32 = 10u;
const UNKNOWN_CANOPY: u32 = 0xffffu;
const SPECIES: u32 = 16u;
const WINDOW: i32 = 8;
// A tree that stands this far into the ground still meets it on a slope.
const SINK_M: f32 = 0.3;
// A tree keeps this share of its crown's radius from a road's edge and from the waterline.
const CLEAR: f32 = 0.8;
// The eye stands in a clearing this wide, so it is never inside a crown.
const EYE_CLEAR_M: f32 = 6.0;
// The ground of a base cell lies within SLOPE_M of its middle's, and no tree is taller than TALLEST_M.
const SLOPE_M: f32 = 60.0;
const TALLEST_M: f32 = 64.0;
// Trees nearer than this stand on drawnHeight, farther ones on the level that draws them: the mesh
// meets it within its morph, below a pixel there.
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

// The canopy code at whole world units: 0 off the forest, UNKNOWN_CANOPY where no tile tells.
fn canopy_at(u: vec2i) -> u32 {
    return canopy_level(u, 0u);
}

// The code of a level of the canopy, whose cells bound the density of the cells under them (canopy.ts).
fn canopy_level(u: vec2i, mip: u32) -> u32 {
    let t = (u >> vec2u(18u)) - data.origin;
    if (any(t < vec2i(0)) || any(t >= vec2i(WINDOW))) {
        return UNKNOWN_CANOPY;
    }
    let layer = data.tiles[t.y * WINDOW + t.x];
    if (layer == 0u) {
        return UNKNOWN_CANOPY;
    }
    return textureLoad(canopy, (u >> vec2u(CANOPY_SHIFT + mip)) & vec2i(255 >> mip), layer - 1u, i32(mip)).x;
}

fn is_forest(code: u32) -> bool {
    return code != 0u && code != UNKNOWN_CANOPY;
}

// The canopy height in metres.
fn canopy_height(code: u32) -> f32 {
    return f32(code & 127u) * 0.5;
}

// Trees per square metre: the stems class, or for a low dense canopy with few stems of 5 m, as many as
// half its cover fills with crowns.
fn density(code: u32) -> f32 {
    let stems_class = (code >> 11u) & 7u;
    let stems = forest.stems[stems_class / 4u][stems_class % 4u];
    let cover = f32((code >> 7u) & 15u) * 0.1;
    let crown = max(1.5, 0.15 * canopy_height(code));
    return max(stems, 0.5 * cover / (3.14159 * crown * crown));
}

// The share of the trees that stand at a distance.
fn kept(d: f32) -> f32 {
    return min(1.0, forest.thin / max(d, 1.0));
}

// The point of LOD space of whole world units plus a part.
fn lod_at(u: vec2i, part: vec2f) -> vec2f {
    return lod_space(vec2f(u - surface.anchor) + part);
}

// The region of the ground at a point, from the finest resident ground page.
fn region_at(s: vec2f) -> u32 {
    for (var k = min(u32(ceil(lod(s))), look.codes.y); k >= COARSEST; k--) {
        let t = texel_of(s, k);
        let whole = t.whole + vec2i(t.part >= vec2f(0.5));
        let page = ground_page(k, whole);
        if (page >= 0) {
            return textureLoad(cells, in_page(page, whole & vec2i(255)), page_layer(page), 0).b;
        }
    }
    return 0u;
}

// The metres from the eye to the nearest point of the cell of a level at a corner (in base cells).
fn nearest(corner: vec2i, level: u32) -> f32 {
    let size = f32(CELL << level);
    let s = lod_at(corner << vec2u(CELL_SHIFT), vec2f(0.5 * size));
    let half = 0.5 * size * surface.unit;
    return length(max(abs(s) - half, vec2f(0.0)));
}

// One thread per base cell of the window: the cell of the coarsest level that holds it and lies far
// enough, listed by its corner's base cell.
@compute @workgroup_size(8, 8)
fn find_cells(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(forest.side))) {
        return;
    }
    let base = forest.origin + vec2i(id.xy);
    var level = LEVELS;
    while (level > 0u && nearest((base >> vec2u(level)) << vec2u(level), level) < forest.thin * f32(1u << level)) {
        level--;
    }
    let corner = (base >> vec2u(level)) << vec2u(level);
    if (any(corner != base)) {
        return;
    }
    let d = nearest(corner, level);
    if (d > forest.reach) {
        return;
    }
    let units = CELL << level;
    let middle = (corner << vec2u(CELL_SHIFT)) + vec2i(units / 2);
    let s = lod_at(middle, vec2f(0.0));
    let ground = units_height(middle, u32(ceil(lod(s))));
    let half = 0.5 * f32(units) * surface.unit;
    // The cell's box: its ground within SLOPE_M per base cell of its middle's, and its trees.
    if (!known(ground) || !in_view(render_at(s, ground + 0.5 * TALLEST_M), 1.5 * half + SLOPE_M * f32(1u << level) + TALLEST_M)) {
        return;
    }
    // The densest canopy that a point of the cell reads, with its jitter of up to half a canopy cell:
    // the canopy's level of the cell's size, and its neighbours.
    var most = 0.0;
    let mip = level + 1u;
    let at = (corner << vec2u(CELL_SHIFT)) >> vec2u(CANOPY_SHIFT + mip);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let code = canopy_level((at + vec2i(i, j)) << vec2u(CANOPY_SHIFT + mip), mip);
            if (is_forest(code)) {
                most = max(most, density(code));
            }
        }
    }
    let base_area = f32(CELL * CELL) * surface.unit * surface.unit;
    let count = min(ceil(most * base_area * kept(d)), f32(forest.points)) * f32(1u << (2u * level));
    if (count < 1.0) {
        return;
    }
    let k = atomicAdd(&found.groups, 1u);
    if (k < arrayLength(&found.list)) {
        found.list[k] = vec4i(corner, i32(u32(count) | (level << 16u)), bitcast<i32>(ground));
    }
}

// The species of a tree of a region at an elevation, of a leaf type (0 any): of those in their band
// with the leaf type, else in their band, else with the leaf type, else of all, by weight.
fn species_of(region: u32, elevation: f32, leaf: u32, pick: f32) -> u32 {
    var totals = vec4f(0.0);
    for (var k = 0u; k < SPECIES; k++) {
        let s = data.species[region * SPECIES + k];
        if (s.w <= 0.0) {
            break;
        }
        let band = elevation >= s.y && elevation <= s.z;
        let same = leaf == 0u || (bitcast<u32>(s.x) >> 16u) == leaf;
        totals += s.w * vec4f(f32(band && same), f32(band), f32(same), 1.0);
    }
    var rule = 3u;
    for (var r = 0u; r < 3u; r++) {
        if (totals[r] > 0.0) {
            rule = r;
            break;
        }
    }
    var left = pick * totals[rule];
    var model = 0xffffffffu;
    for (var k = 0u; k < SPECIES; k++) {
        let s = data.species[region * SPECIES + k];
        if (s.w <= 0.0) {
            break;
        }
        let band = elevation >= s.y && elevation <= s.z;
        let same = leaf == 0u || (bitcast<u32>(s.x) >> 16u) == leaf;
        if (select(select(true, same, rule == 2u), select(band, band && same, rule == 0u), rule < 2u)) {
            model = bitcast<u32>(s.x) & 0xffffu;
            left -= s.w;
            if (left < 0.0) {
                break;
            }
        }
    }
    return model;
}

// Whether the feature mask keeps a tree off a point: on a road or nearer to it than `clear` metres, in
// water or nearer to it, on a building.
fn off_ground(s: vec2f, clear: f32) -> bool {
    let mask = mask_at(s, u32(ceil(lod(s))), false);
    if (mask.level == 0u) {
        return false;
    }
    let road = mask.kind.x != 0u && mask_metres(mask.shape.x) < clear;
    return road || mask_metres(mask.shape.y) < clear || (mask.kind.z & BUILDING) != 0u;
}

// Point r of a base cell, if a tree stands there: emitted on the drawn surface. `ground_near` is the
// height of the middle of the listed cell, `slope` how far its ground may lie from it, and `region`
// the region there.
fn tree(cell: vec2i, r: u32, ground_near: f32, slope: f32, region: u32) {
    let corner = cell << vec2u(CELL_SHIFT);
    let h = mix_bits3(cell.x, cell.y, r + 3u);
    let shift = vec2f(unit_float(mix_bits3(cell.x, cell.y, 1u)), unit_float(mix_bits3(cell.x, cell.y, 2u)));
    let part = fract(data.points[r] + shift) * f32(CELL);
    let at = corner + vec2i(part);
    // The canopy cell that a tree reads moves by up to half a cell, so the forest's edge does not follow the cells.
    let jitter = vec2i((vec2f(unit_float(mix_bits(h)), unit_float(mix_bits(h ^ 0x9e3779b9u))) - 0.5) * f32(1 << CANOPY_SHIFT));
    let code = canopy_at(at + jitter);
    if (!is_forest(code)) {
        return;
    }
    let s = lod_at(corner, part);
    let share = kept(length(s));
    let n = density(code) * f32(CELL * CELL) * surface.unit * surface.unit * share;
    if (f32(r) >= n) {
        return;
    }
    let height = max(3.0, canopy_height(code) * (0.7 + 0.35 * unit_float(mix_bits(h + 13u))));
    // On the cell's ground, a tree out of view stays out of view on its own.
    if (length(s) < EYE_CLEAR_M || !in_view(render_at(s, ground_near + 0.5 * height), slope + height)) {
        return;
    }
    var ground: f32;
    if (length(s) < MESH_HEIGHT_M) {
        ground = plain_height(s);
    } else {
        ground = units_height(at, u32(ceil(lod(s))));
    }
    if (!known(ground)) {
        return;
    }
    // The leaf type: a mixed cell is half of each; an unknown one takes any species.
    var leaf = (code >> 14u) & 3u;
    if (leaf == 3u) {
        leaf = select(1u, 2u, unit_float(mix_bits(h + 7u)) < 0.5);
    }
    let model = species_of(region, ground, leaf, unit_float(mix_bits(h + 11u)));
    // Kept trees stand wider, so the forest keeps its cover as it thins.
    let width = min(2.0, inverseSqrt(share));
    // A road or a path is a lane through the forest: crowns reach a little over it.
    if (model == 0xffffffffu || off_ground(s, CLEAR * models[model].spread * height * width)) {
        return;
    }
    let yaw = 6.2831853 * unit_float(mix_bits(h + 17u));
    emit(model, render_at(s, ground - SINK_M), height, yaw, width, 0.88 + 0.24 * unit_float(mix_bits(h + 19u)), min(1.0, n - f32(r)));
}

// The base cell of rank R of a cell of a level: its low 2 × level bits interleave x and y.
fn base_of(rank: u32, level: u32) -> vec2i {
    var at = vec2u(0u);
    for (var b = 0u; b < level; b++) {
        at |= vec2u((rank >> (2u * b)) & 1u, (rank >> (2u * b + 1u)) & 1u) << vec2u(b);
    }
    return vec2i(at);
}

var<workgroup> cell_region: u32;

@compute @workgroup_size(64)
fn trees(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    if (group.x >= arrayLength(&listed.list)) {
        return;
    }
    let entry = listed.list[group.x];
    let count = u32(entry.z) & 0xffffu;
    let level = u32(entry.z) >> 16u;
    let slope = SLOPE_M * f32(1u << level);
    // The cell takes the region of its middle: regions are far wider than cells.
    if (lane == 0u) {
        cell_region = region_at(lod_at(entry.xy << vec2u(CELL_SHIFT), vec2f(f32(CELL << level) * 0.5)));
    }
    let region = workgroupUniformLoad(&cell_region);
    for (var rank = lane; rank < count; rank += 64u) {
        tree(entry.xy + base_of(rank, level), rank >> (2u * level), bitcast<f32>(entry.w), slope, region);
    }
}
