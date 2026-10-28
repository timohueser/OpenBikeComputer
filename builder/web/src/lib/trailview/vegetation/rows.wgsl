// The plantings: the plants of the materials planted in rows, on the rows of the material fields
// (ground/fields.wgsl), placed on the GPU each frame with no stored position.
//
// One workgroup per base cell, half a field cell, laid so that each point of it looks at the same
// 2 × 2 field cells. A cell samples which crops its ground holds, then walks the row points of each
// crop's fields that may lie in it. A crop thins past THIN times the square root of the area of a
// plant: point (i, j) of a field, i along the rows and j across them, stands to that distance times
// 2^r, where 2^r divides both i and j. So the far rows keep every 2^r-th plant of every 2^r-th row, a
// plant that stands far away stands near too, and a dense crop stands as many plants as a sparse one.
// #include ../terrain/drawn-mesh.wgsl
// #include ../ground/look.wgsl
// #include ../instances/emit.wgsl
// #include clearing.wgsl

struct Window {
    // The first base cell, in base cells of the world, its cells a side, and the farthest reach of a crop.
    origin: vec2i,
    side: u32,
    reach: f32,
}

// A crop: its material, the metres from the eye where its plants stand, and their heights.
struct Crop {
    material: u32,
    reach: f32,
    height: vec2f,
}

// Per material code its crop, NO_CROP for none; the crops; per region the model word of each crop
// (rows.ts): the model in the low 16 bits, NO_MODEL for none; WINDWARD for a model that keeps its
// heading, ALONG for one that runs along its row, else it turns at random; from bit 18 its shapes,
// consecutive models.
struct Data {
    crop_of: array<vec4u, 16>,
    crops: array<Crop, CROPS>,
    models: array<vec4u>,
}

@group(0) @binding(8) var<storage, read> data: Data;
@group(0) @binding(9) var<uniform> window: Window;

const CROPS: u32 = 4u;
const NO_CROP: u32 = 0xffffffffu;
const NO_MODEL: u32 = 0xffffu;
const WINDWARD: u32 = 0x10000u;
const ALONG: u32 = 0x20000u;
// Units along a base cell: half a field cell.
const BASE_SHIFT: u32 = FIELD_SHIFT - 1u;
const BASE: i32 = FIELD / 2;
// A crop thins past THIN times the square root of the area of a plant: about π THIN² plants stand
// unthinned about the eye, whatever the crop.
const THIN: f32 = 30.0;
// A plant fades out over this share of its own distance.
const FADE: f32 = 0.1;
// The ground of a base cell lies within SLOPE_M of its middle's, and no plant is taller than TALLEST_M.
const SLOPE_M: f32 = 40.0;
const TALLEST_M: f32 = 12.0;
// The row points that a cell walks per field at most, and the lanes that walk them: a near cell holds
// thousands, and the few near cells take the longest.
const MOST: u32 = 8192u;
const LANES: u32 = 256u;

var<workgroup> cell_ground: f32;
var<workgroup> found: atomic<u32>;
var<workgroup> crops_here: u32;

fn lod_at(u: vec2i) -> vec2f {
    return lod_space(vec2f(u - surface.anchor));
}

fn plant_hash(u: vec2i, salt: u32) -> f32 {
    return unit_float(mix_bits3(u.x, u.y, salt));
}

@compute @workgroup_size(LANES)
fn plantings(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let base = window.origin + vec2i(group.xy);
    // Half a field cell past a whole one, so the cell's points look at the field cells from base / 2.
    let corner = (base << vec2u(BASE_SHIFT)) + vec2i(BASE);
    let middle = corner + vec2i(BASE / 2);
    let centre_s = lod_at(middle);
    let half = 0.5 * f32(BASE) * surface.unit;
    let nearest = length(max(abs(centre_s) - half, vec2f(0.0)));
    if (nearest > window.reach) {
        return;
    }
    // One lane culls the cell: its box holds its ground within SLOPE_M of its middle's, and its plants.
    if (lane == 0u) {
        let height = units_height(middle, u32(ceil(lod(centre_s))));
        let radius = 1.5 * half + SLOPE_M + TALLEST_M;
        let centre = render_at(centre_s, height + 0.5 * TALLEST_M);
        cell_ground = select(NONE, height, known(height) && visible(centre, radius) && !hidden(centre, radius));
        atomicStore(&found, 0u);
    }
    let ground = workgroupUniformLoad(&cell_ground);
    if (!known(ground)) {
        return;
    }
    // The crops of the cell: its ground on an 8 × 8 grid.
    if (lane < 64u) {
        let sampled = finest_cell(lod_at(corner + vec2i((vec2f(f32(lane % 8u), f32(lane / 8u)) + 0.5) * f32(BASE / 8))));
        let crop = data.crop_of[min(sampled.r, 63u) / 4u][min(sampled.r, 63u) % 4u];
        if (sampled.r < 64u && crop < CROPS && nearest <= data.crops[crop].reach) {
            atomicOr(&found, 1u << crop);
        }
    }
    workgroupBarrier();
    if (lane == 0u) {
        crops_here = atomicLoad(&found);
    }
    let here = workgroupUniformLoad(&crops_here);
    let first = base >> vec2u(1u);
    for (var c = 0u; c < CROPS; c++) {
        if ((here & (1u << c)) == 0u) {
            continue;
        }
        let crop = data.crops[c];
        let rows = rule(0u, slot_rows() + i32(crop.material));
        for (var k = 0; k < 4; k++) {
            let cell = first + vec2i(k & 1, k >> 1);
            let f = field_rows(cell, rows);
            let across = vec2f(-f.along.y, f.along.x);
            let site = field_site(cell);
            // Only every 2^level-th point stands in a cell this far, as THIN keeps them.
            let thin = THIN * sqrt(f.pitch.x * f.pitch.y);
            let stride = i32(1u << u32(clamp(floor(log2(max(nearest, 1.0) / thin)), 0.0, 16.0)));
            // The row points about the cell's square: its corners in metres from the site, along and across the rows.
            let q = vec2f(corner - site) * f.unit;
            let size = f32(BASE) * f.unit;
            var low = vec2f(1e30);
            var high = vec2f(-1e30);
            for (var n = 0; n < 4; n++) {
                let p = q + size * vec2f(f32(n & 1), f32(n >> 1));
                let r = vec2f(dot(p, f.along), dot(p, across)) / f.pitch - 0.5;
                low = min(low, r);
                high = max(high, r);
            }
            let start = vec2i(ceil(low / f32(stride)));
            let count = max(vec2i(floor(high / f32(stride))) - start + 1, vec2i(0));
            let total = min(u32(count.x * count.y), MOST);
            for (var t = lane; t < total; t += LANES) {
                let index = (start + vec2i(i32(t % u32(count.x)), i32(t / u32(count.x)))) * stride;
                let offset = (f32(index.x) + 0.5) * f.pitch.x * f.along + (f32(index.y) + 0.5) * f.pitch.y * across;
                let at = site + vec2i(round(offset / f.unit));
                if (any(at < corner) || any(at >= corner + vec2i(BASE))) {
                    continue;
                }
                plant(c, cell, f, index, at, thin, ground, crop);
            }
        }
    }
}

// Row point `index` of field `cell` at whole world units `at`, if a plant of crop c stands there.
// `ground_near` is the height of the middle of its base cell.
fn plant(c: u32, cell: vec2i, f: FieldRows, index: vec2i, at: vec2i, thin: f32, ground_near: f32, crop: Crop) {
    let s = lod_at(at);
    let d = length(s);
    let rank = f32(min(countTrailingZeros(index.x), countTrailingZeros(index.y)));
    let appear = min(clamp((1.0 - d / (thin * exp2(rank))) / FADE, 0.0, 1.0), clamp((1.0 - d / crop.reach) / FADE, 0.0, 1.0));
    if (appear <= 0.0) {
        return;
    }
    let field = field_of(field_cells(vec2f(at - surface.anchor)), vec2f(at - surface.anchor));
    if (any(field.cell != cell) || field.edge * f.unit < HEADLAND_M) {
        return;
    }
    let t = finest_texel(s);
    if (t.z < 0) {
        return;
    }
    let ground = textureLoad(cells, t.xy, t.z, 0);
    if (ground.r != crop.material || ground.b >= arrayLength(&data.models)) {
        return;
    }
    let height = mix(crop.height.x, crop.height.y, field_hash(cell, 4u)) * (0.92 + 0.16 * plant_hash(at, 1u));
    let centre = render_at(s, ground_near + 0.5 * height);
    if (!visible(centre, SLOPE_M + height) || hidden(centre, SLOPE_M + height)) {
        return;
    }
    let word = data.models[ground.b][c];
    var model = word & 0xffffu;
    if (model == NO_MODEL) {
        return;
    }
    model += mix_bits3(at.x, at.y, 2u) % max(word >> 18u, 1u);
    let m = models[model];
    if (kept_off(s, m.spread * height)) {
        return;
    }
    let stood = stand_ground(s, at);
    if (!known(stood.x)) {
        return;
    }
    let low = stood.x + height * max(m.crown, 0.0);
    let spread = EYE_CLEAR_SPREAD_M * plant_hash(at, 3u);
    if (crown_distance(s, low, stood.x + height, m.spread * height) < EYE_CLEAR_M + spread) {
        return;
    }
    // The row's direction in LOD space, y north.
    var yaw = 6.2831853 * plant_hash(at, 4u);
    if ((word & ALONG) != 0u) {
        yaw = atan2(-f.along.y, f.along.x);
    } else if ((word & WINDWARD) != 0u) {
        yaw = 0.0;
    }
    // Kept plants stand wider, so a thinned crop keeps its cover; the kept plants of a hedge, about
    // d / thin pitches apart, touch along it.
    let kept = max(d / thin, 1.0);
    let hedge = rule(0u, slot_traits() + i32(crop.material)).z > 0.5;
    let touch = select(1.0, kept * f.pitch.x / (2.0 * m.spread * height), hedge);
    let width = clamp(max(kept, touch), 1.0, 2.0);
    let coarse = units_height(at, BAND_LEVEL);
    emit_tree(model, s, stood.x, height, yaw, width, 0.9 + 0.2 * plant_hash(at, 5u), appear, 0.0, stood.y, plant_hash(at, 6u),
        smoothstep(0.25, 0.75, textureLoad(cache, t.xy, t.z * CACHE_PLANES, 0).a), ground_albedo(t), select(stood.x, coarse, known(coarse)));
}
