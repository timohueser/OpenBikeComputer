// The ground look: a port of the ground shading of the prototype (`groundOn`, `groundAt` and
// `terrainFragment` of src/shaders.ts) over the ground pages. A node draws the ground of its level.
// The rules run once per texel when a page arrives (ground.wgsl); a pixel reads their result
// filtered. What the prototype measured from its fixed spot, this measures from the eye.
// #include ../terrain/drawn-height.wgsl
// #include ../light/shadow.wgsl
// #include roads.wgsl
// #include fields.wgsl
// #include ../dressing/rocks.wgsl

// Per ground node MAP × MAP entries per level about the eye, in the windows of the LOD map: the page + 1, or 0.
@group(2) @binding(1) var<storage, read> ground_map: array<u32>;
// Per texel: the material, the lithology, the region and barren × 255.
@group(2) @binding(2) var cells: texture_2d_array<u32>;
// Per region a row of slots: ground/rules.ts.
@group(2) @binding(3) var rules: texture_2d<f32>;
@group(2) @binding(4) var filtered: sampler;
// Per page CACHE_PLANES layers, ground.wgsl: the albedo (square root) and the snow before its edge;
// tree, the two patches and green; bare_from, barren, blades and banded.
@group(2) @binding(5) var cache: texture_2d_array<f32>;
// A table of 256 × 256 hash values that repeats, for value noise in one fetch.
@group(2) @binding(6) var noise_table: texture_2d<f32>;
// Per page BEARINGS / 4 layers of HORIZON × HORIZON texels, ground.wgsl: the horizon's altitude in
// BEARINGS bearings clockwise from north, 4 per layer, as a share of HORIZON_TOP.
@group(2) @binding(7) var horizons: texture_2d_array<f32>;
// The relief, ground.wgsl: per texel its own openness; per COARSE_STEP × COARSE_STEP texels one texel,
// the normal two levels coarser (xy as 0.5 + 0.5 n) and its openness.
@group(2) @binding(11) var relief_open: texture_2d_array<f32>;
@group(2) @binding(12) var relief_coarse: texture_2d_array<f32>;

// A node coarser than level 16 that its distance keeps from splitting (terrain.ts) draws without the
// road paint, the blades and the bushes' dots: the compiler drops them, and with them the registers
// that they take from every pixel. Such a node lies at least RANGE.least nodes of level 16 from the
// eye (lod.ts). At 71° N on a canvas 2880 pixels tall, an output pixel there spans 0.37 m or more at
// the screen's centre, and 0.22 m at a corner of a 16:9 frame. That is past the fade of the blades
// (0.18 m). The dots of a patch fade at 0.1 × size_m: the 3 m that rules.py allows fade before such a
// node up to 64° N. Blotches do not fade: rules.py keeps them two such pixels across.
override DETAIL: bool = true;
// A corridor's band: the ground beside a road, shaped by it, under no paint and with facets of its own.
override BAND: bool = false;

const COVER_TREE: u32 = 0u;
const COVER_SHRUB: u32 = 1u;
const COVER_GRASS: u32 = 2u;
const COVER_CROP: u32 = 3u;
const COVER_BUILT: u32 = 4u;
const COVER_BARE: u32 = 5u;
const COVER_SNOW: u32 = 6u;
const COVER_WATER: u32 = 7u;
const COVER_WETLAND: u32 = 8u;
const COVER_MOSS: u32 = 10u;

// Pages per layer side of the ground textures.
const GROUND_SIDE: i32 = 4;
const CACHE_PLANES: i32 = 3;
// Texels a side of a texel of `relief_coarse`, and those texels a side of a page.
const COARSE_STEP: i32 = 2;
const COARSE_PAGE: i32 = 256 / COARSE_STEP;
// Horizon texels a side of a page, each over HORIZON_SPAN × HORIZON_SPAN of its texels; the bearings
// of a horizon texel, and the highest horizon in radians (75°).
const HORIZON: i32 = 32;
const HORIZON_SPAN: i32 = 8;
const BEARINGS: u32 = 16u;
const HORIZON_TOP: f32 = 1.309;
// Radians of the sun's altitude across the soft edge of a shadow: the sun's disc, and the spread of a
// horizon between its texels and its bearings.
const PENUMBRA: f32 = 0.017;

struct Ground {
    albedo: vec3f,
    snow: f32,
    tree: f32,
    // The share of the ground under each patch of the region; where the satellite sees green on
    // ground that is not bare rock.
    patches: vec2f,
    green: f32,
    // The slope (normal z) below which the ground is bare rock; land that no quarter sees green; the
    // sward's share where it grows blades; the rock's banded share.
    bare_from: f32,
    barren: f32,
    blades: f32,
    banded: f32,
    open: f32,
    // The share of the sun that the horizons let through.
    sun: f32,
    region: u32,
    lithology: u32,
    material: u32,
    // Where the page reads it: the level, the page (< 0 for none), its corner and the point in texels from it.
    level: u32,
    page: i32,
    base: vec2i,
    p: vec2f,
}

// Dave Hoskins' hash.
fn hash(q: vec2f) -> f32 {
    var p3 = fract(q.xyx * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Smooth value noise over the table: the bilinear fetch at a point moved by the smooth step between
// two texels interpolates as smoothly as four hashes would.
fn noise(q: vec2f) -> f32 {
    let i = floor(q);
    let f = q - i;
    return textureSampleLevel(noise_table, filtered, (i + f * f * (3.0 - 2.0 * f) + 0.5) / 256.0, 0.0).r;
}

fn rule(region: u32, slot: i32) -> vec4f {
    return textureLoad(rules, vec2i(slot, i32(region)), 0);
}

// Whether a set of materials, as `bits` of rules/regions.ts makes it, holds a material code.
// A code of 64 or more, such as the NO_CELL of a page that is not resident, is in no set.
fn holds(materials: vec2u, material: u32) -> bool {
    return material < 64u && ((materials[material >> 5u] >> (material & 31u)) & 1u) == 1u;
}

// The slots of ground/rules.ts, from the sizes of the vocabulary.
fn slot_materials() -> i32 { return 33; }
fn slot_traits() -> i32 { return 33 + i32(look.codes.y); }
fn slot_lithologies() -> i32 { return 33 + 2 * i32(look.codes.y); }
fn slot_cliff() -> i32 { return slot_lithologies() + i32(look.codes.z); }
fn slot_roads() -> i32 { return slot_cliff() + 10; }
fn slot_kerb() -> i32 { return slot_cliff() + 18; }
fn slot_road_line() -> i32 { return slot_cliff() + 19; }
fn slot_concrete() -> i32 { return slot_cliff() + 20; }
fn slot_setts() -> i32 { return slot_cliff() + 21; }
fn slot_zebra() -> i32 { return slot_cliff() + 22; }
fn slot_zebra_setts() -> i32 { return slot_cliff() + 23; }
fn slot_give_way() -> i32 { return slot_cliff() + 24; }
fn slot_dashes() -> i32 { return slot_cliff() + 25; }
fn slot_kinds() -> i32 { return slot_cliff() + 26; }
fn slot_hardness() -> i32 { return slot_kinds() + i32(look.codes.y); }
fn slot_rows() -> i32 { return slot_hardness() + i32(look.codes.z); }

// The cover's ramp in the region, low NDVI to high.
fn ramp(region: u32, cover: u32, t: f32) -> vec3f {
    let slot = i32(cover) * 3 + select(1, 0, t < 0.5);
    return mix(rule(region, slot).rgb, rule(region, slot + 1).rgb, select(2.0 * t - 1.0, 2.0 * t, t < 0.5));
}

// What one texel is through its region's rules.
struct Parts {
    palette: vec3f,
    paint: vec4f,
    water: f32,
    tree: f32,
    patches: vec2f,
    sward: f32,
    bare: f32,
    banded: f32,
    hardness: f32,
}

fn texel_parts(cell: vec4u, t: f32, ledge: f32, barren: f32, decided: bool) -> Parts {
    let own = cell.b;
    let kinds = rule(own, slot_kinds() + i32(cell.r));
    var cover = u32(kinds.x + 0.5);
    // Where the snow data or the satellite looks, it decides snow.
    if (cover == COVER_SNOW && decided) {
        cover = COVER_BARE;
    }
    let open = cover == COVER_SHRUB || cover == COVER_GRASS || cover == COVER_WETLAND || cover == COVER_MOSS;
    var c = ramp(own, cover, t);
    // Open land that no quarter sees green is bare, whatever WorldCover calls it.
    if (open && barren > 0.0) {
        c = mix(c, ramp(own, COVER_BARE, t), barren);
    }
    // Exposed rock where the satellite sees green is a ledge of turf: the region's turf paint where it has one, else grass.
    let turf = kinds.z * ledge;
    if (turf > 0.0) {
        c = mix(c, ramp(own, COVER_GRASS, t), turf);
    }
    let named = rule(own, slot_materials() + i32(cell.r));
    var parts: Parts;
    parts.palette = c;
    parts.paint = named.a * (1.0 - turf) * vec4f(named.rgb, 1.0);
    if (turf > 0.0) {
        let paint = rule(own, slot_cliff() + 9);
        parts.paint += paint.a * turf * vec4f(paint.rgb, 1.0);
    }
    parts.water = select(0.0, 1.0, cover == COVER_WATER);
    parts.tree = select(0.0, 1.0, cover == COVER_TREE);
    let under = u32(kinds.y + 0.5);
    parts.patches = vec2f(f32(under & 1u), f32(under >> 1u));
    parts.sward = select(1.0, 0.0, cover == COVER_BARE || cover == COVER_BUILT || cover == COVER_SNOW || cover == COVER_WATER || cover == COVER_MOSS);
    parts.bare = kinds.z;
    parts.banded = rule(own, slot_lithologies() + i32(cell.g)).a;
    parts.hardness = rule(own, slot_hardness() + i32(cell.g)).x;
    return parts;
}

// The albedo of the parts, with the satellite's chroma rescaled to the palette's luma, bare_luma,
// and the paint of the material.
fn albedo_of(parts: Parts, region: u32, material: u32, sat: vec4f, barren: f32) -> vec3f {
    var albedo = parts.palette;
    var painted = parts.paint.rgb / max(parts.paint.a, 1e-4);
    if (sat.a > 0.0) {
        // The satellite gives chroma only: brightness comes from the palette and the sun, because the
        // mosaics carry their own shadows.
        let shown = pow(min(sat.rgb * 1.65, vec3f(1.0)), vec3f(2.2));
        let chroma = shown * (luma(parts.palette) / max(luma(shown), 1e-4));
        albedo = mix(mix(vec3f(luma(chroma)), chroma, 0.8), parts.palette, rule(region, slot_cliff() + 5).x);
        // Barren ground is as light as the satellite sees it against the region's bare_luma.
        let bare_luma = rule(region, slot_cliff() + 4).a;
        if (bare_luma > 0.0) {
            albedo *= mix(1.0, clamp(luma(pow(sat.rgb, vec3f(2.2))) / bare_luma, 0.6, 1.6), barren);
        }
        // The paint keeps its material's share of the satellite's hue, at its own brightness.
        let keep = select(0.0, rule(region, slot_traits() + i32(material)).x, parts.paint.a > 0.0);
        if (keep > 0.0) {
            painted = mix(painted, shown * (luma(painted) / max(luma(shown), 1e-4)), keep);
        }
    }
    return mix(albedo, painted, parts.paint.a * (1.0 - parts.water));
}

// The satellite of a colour texel: the colour, and 1 where a quarter looked; the NDVI.
fn satellite(texel: vec4f) -> vec4f {
    let code = texel.a * 255.0;
    return vec4f(texel.rgb, select(0.0, 1.0, code >= 1.0));
}

fn ndvi_of(texel: vec4f) -> f32 {
    return select(0.45, (texel.a * 255.0 - 1.0) / 127.0 - 1.0, texel.a * 255.0 >= 1.0);
}

// The ground page of the node of a level that holds texel t, or -1.
fn ground_page(level: u32, t: vec2i) -> i32 {
    let slot = (t >> vec2u(8u)) - surface.levels[level].xy;
    if (any(slot < vec2i(0)) || any(slot >= vec2i(MAP))) {
        return -1;
    }
    return i32(ground_map[(i32(level) * MAP + slot.y) * MAP + slot.x]) - 1;
}

// The finest resident ground page of a level or coarser that holds a point: its level, the point's
// texel there and the page, -1 where none holds it.
struct Held {
    level: u32,
    t: Texel,
    page: i32,
}

fn held_at(s: vec2f, level: u32) -> Held {
    var k = min(level, look.codes.x);
    var t = texel_of(s, k);
    var page = ground_page(k, t.whole + vec2i(t.part >= vec2f(0.5)));
    while (page < 0 && k > COARSEST) {
        k--;
        t = texel_of(s, k);
        page = ground_page(k, t.whole + vec2i(t.part >= vec2f(0.5)));
    }
    return Held(k, t, page);
}

// The ground cell at a point from the finest resident ground page of a level or coarser: r the
// material, g the lithology, b the region.
fn cell_at(s: vec2f, level: u32) -> vec4u {
    let held = held_at(s, level);
    if (held.page < 0) {
        return vec4u(0u);
    }
    let whole = held.t.whole + vec2i(held.t.part >= vec2f(0.5));
    return textureLoad(cells, in_page(held.page, whole & vec2i(255)), page_layer(held.page), 0);
}

// The texel of a point in the finest ground pages, as `in_page` places it, and its page's layer; z is -1
// where that page is not resident. What stands by it stays as finer pages replace coarser ones: a page
// that arrives adds things but changes none.
fn finest_texel(s: vec2f) -> vec3i {
    let t = texel_of(s, look.codes.x);
    let whole = t.whole + vec2i(t.part >= vec2f(0.5));
    let page = ground_page(look.codes.x, whole);
    if (page < 0) {
        return vec3i(0, 0, -1);
    }
    return vec3i(in_page(page, whole & vec2i(255)), page_layer(page));
}

// The albedo of the ground at a texel of finest_texel.
fn ground_albedo(t: vec3i) -> vec3f {
    let c = textureLoad(cache, t.xy, t.z * CACHE_PLANES, 0).rgb;
    return c * c;
}

// The ground cell at a point from the finest ground pages (finest_texel), as `cell_at`; NO_CELL where
// that page is not resident.
fn finest_cell(s: vec2f) -> vec4u {
    let t = finest_texel(s);
    if (t.z < 0) {
        return NO_CELL;
    }
    return textureLoad(cells, t.xy, t.z, 0);
}

const NO_CELL: vec4u = vec4u(0xffffffffu);
// Trees stand within TREE_REACH_M of the eye, as in tree-models.ts. Past it, the floor of woodland
// stands for their crowns over CROWN_SHARE of the snow: the owner's choice by eye.
const TREE_REACH_M: f32 = 4500.0;
const CROWN_SHARE: f32 = 0.5;

// The region of the ground at a point, from the finest resident ground page.
fn region_at(s: vec2f) -> u32 {
    return cell_at(s, u32(ceil(lod(s)))).b;
}

// The snow on the ground at a point on the date, 0 to 1, from the finest resident ground page.
fn snow_at(s: vec2f) -> f32 {
    return snow_held(held_at(s, u32(ceil(lod(s)))));
}

fn snow_held(held: Held) -> f32 {
    if (held.page < 0) {
        return 0.0;
    }
    let whole = held.t.whole + vec2i(held.t.part >= vec2f(0.5));
    return smoothstep(0.25, 0.75, textureLoad(cache, in_page(held.page, whole & vec2i(255)), page_layer(held.page) * CACHE_PLANES, 0).a);
}

// The ground's albedo in a held page, from the satellite on the date; 0 where no page is resident.
fn albedo_held(held: Held) -> vec3f {
    if (held.page < 0) {
        return vec3f(0.0);
    }
    let whole = held.t.whole + vec2i(held.t.part >= vec2f(0.5));
    let c = textureLoad(cache, in_page(held.page, whole & vec2i(255)), page_layer(held.page) * CACHE_PLANES, 0).rgb;
    return c * c;
}

// The share of the sun that the horizons of a held page let through, 1 where no page holds the point.
fn sun_held(held: Held) -> f32 {
    if (held.page < 0) {
        return 1.0;
    }
    // Texel coordinates from the page's corner, in horizon texels.
    let base = (held.t.whole + vec2i(held.t.part >= vec2f(0.5))) & vec2i(~255);
    let q = clamp((vec2f(held.t.whole - base) + held.t.part + 0.5) / f32(HORIZON_SPAN), vec2f(0.5), vec2f(f32(HORIZON) - 0.5));
    let uv = (vec2f(in_horizon(held.page, vec2i(0))) + q) / f32(HORIZON * GROUND_SIDE);
    let way = look.sun.w * f32(BEARINGS);
    let k = u32(way) % BEARINGS;
    let next = (k + 1u) % BEARINGS;
    let layer = i32(BEARINGS / 4u) * page_layer(held.page);
    let a = textureSampleLevel(horizons, filtered, uv, layer + i32(k / 4u), 0.0)[k % 4u];
    let b = textureSampleLevel(horizons, filtered, uv, layer + i32(next / 4u), 0.0)[next % 4u];
    let horizon = mix(a, b, fract(way)) * HORIZON_TOP;
    return smoothstep(-PENUMBRA, PENUMBRA, asin(clamp(look.sun.z, -1.0, 1.0)) - horizon);
}

// The share of the sun at a point, from the finest resident ground page.
fn sun_at(s: vec2f) -> f32 {
    return sun_held(held_at(s, u32(ceil(lod(s)))));
}

// Where horizon texel c of a page lies in its layers.
fn in_horizon(page: i32, c: vec2i) -> vec2i {
    return c + HORIZON * vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE);
}

fn in_page(page: i32, c: vec2i) -> vec2i {
    return clamp(c, vec2i(0), vec2i(255)) + 256 * vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE);
}

// Where texel c of a page lies in the layers of `relief_coarse`.
fn in_coarse(page: i32, c: vec2i) -> vec2i {
    return c + COARSE_PAGE * vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE);
}

fn page_layer(page: i32) -> i32 {
    return page / (GROUND_SIDE * GROUND_SIDE);
}

// A filtered read at a point `local` texels from the page's corner, kept inside the page of `side` texels.
fn page_uv(page: i32, local: vec2f, side: f32) -> vec2f {
    return (side * vec2f(vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE)) + clamp(local, vec2f(0.5), vec2f(side - 0.5))) / (side * f32(GROUND_SIDE));
}

// Texel `local` of a level, counted from the corner `base` of page `home`: in its own page when that
// page is resident, else clamped into `home`. The texel in the layer, and the layer.
fn texel_at(level: u32, home: i32, base: vec2i, local: vec2i) -> vec3i {
    var page = home;
    var at = local;
    if (any(local < vec2i(0)) || any(local > vec2i(255))) {
        let other = ground_page(level, base + local);
        if (other >= 0) {
            page = other;
            at = local & vec2i(255);
        }
    }
    return vec3i(in_page(page, at), page_layer(page));
}

// The planes of the cache at point p of a level, in texels from the corner `base` of page `home`
// (texel centres are whole numbers), and in the w of a fourth the texel's own openness. Bilinear across the page borders: the sampler inside the page, four
// loads where the footprint crosses a border.
fn cache_read(level: u32, home: i32, base: vec2i, p: vec2f) -> array<vec4f, 4> {
    var out: array<vec4f, 4>;
    let c = vec2i(floor(p));
    if (all(c >= vec2i(0)) && all(c < vec2i(255))) {
        let uv = page_uv(home, p + 0.5, 256.0);
        for (var plane = 0; plane < CACHE_PLANES; plane++) {
            out[plane] = textureSampleLevel(cache, filtered, uv, page_layer(home) * CACHE_PLANES + plane, 0.0);
        }
        out[3].w = textureSampleLevel(relief_open, filtered, uv, page_layer(home), 0.0).r;
    } else {
        let f = p - floor(p);
        for (var j = 0; j < 4; j++) {
            let o = vec2i(j & 1, j >> 1);
            let at = texel_at(level, home, base, c + o);
            let w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
            for (var plane = 0; plane < CACHE_PLANES; plane++) {
                out[plane] += w * textureLoad(cache, at.xy, at.z * CACHE_PLANES + plane, 0);
            }
            out[3].w += w * textureLoad(relief_open, at.xy, at.z, 0).r;
        }
    }
    return out;
}

// The coarse relief at p, as cache_read: xy the coarse normal, z its openness.
fn coarse_read(level: u32, home: i32, base: vec2i, p: vec2f) -> vec3f {
    let step = f32(COARSE_STEP);
    let q = (p - 0.5 * (step - 1.0)) / step;
    let c = vec2i(floor(q));
    if (all(c >= vec2i(0)) && all(c < vec2i(COARSE_PAGE - 1))) {
        return textureSampleLevel(relief_coarse, filtered, page_uv(home, q + 0.5, f32(COARSE_PAGE)), page_layer(home), 0.0).rgb;
    }
    let f = q - floor(q);
    var out = vec3f(0.0);
    for (var j = 0; j < 4; j++) {
        let o = vec2i(j & 1, j >> 1);
        // texel_at places page texels: the first of those of coarse texel c + o.
        let at = texel_at(level, home, base, COARSE_STEP * (c + o));
        let w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        out += w * textureLoad(relief_coarse, at.xy / COARSE_STEP, at.z, 0).rgb;
    }
    return out;
}

// The ground under a point of a node of `level`, from the page of its level or the next coarser one
// that is resident. `world` is the point in the frame, for the brush of the texel edges. The satellite decides snow only where the data has none, and a snow edge is crisper than the texels'
// blend across it.
fn ground_at(level: u32, s: vec2f, world: vec2f, footprint: f32) -> Ground {
    let held = held_at(s, level);
    let k = held.level;
    let t = held.t;
    let page = held.page;
    var g: Ground;
    g.page = page;
    if (page < 0) {
        // No level has ground here yet: the palette's grass of the first region.
        g.albedo = ramp(0u, COVER_GRASS, 0.5);
        g.open = 1.0;
        g.bare_from = 0.62;
        g.sun = 1.0;
        return g;
    }
    // Texel coordinates from the page's corner: texel centres are whole numbers.
    let base = (t.whole + vec2i(t.part >= vec2f(0.5))) & vec2i(~255);
    let texel = vec2f(t.whole - base) + t.part;
    var p = texel;
    // A wobble of up to half a texel turns the texel stairs into brush edges, where a texel spans pixels.
    let metres = surface.unit * f32(1 << (22u - k));
    if (footprint < metres) {
        let q = world / metres * 0.7;
        p += 0.45 * (vec2f(noise(q), noise(q + 31.0)) * 2.0 - 1.0);
    }
    let planes = cache_read(k, page, base, p);
    g.albedo = planes[0].rgb * planes[0].rgb;
    g.snow = smoothstep(0.25, 0.75, planes[0].a);
    g.tree = planes[1].r;
    g.patches = planes[1].gb;
    g.green = planes[1].a;
    g.bare_from = planes[2].r;
    g.barren = planes[2].g;
    g.blades = planes[2].b;
    g.banded = planes[2].a;
    g.open = 0.6 + 0.55 * planes[3].a;
    g.level = k;
    g.base = base;
    g.p = p;
    g.sun = sun_held(held);
    let cell = textureLoad(cells, in_page(page, vec2i(floor(texel + 0.5))), page_layer(page), 0);
    g.region = cell.b;
    g.lithology = cell.g;
    g.material = cell.r;
    return g;
}

// The normal in render space and the openness of the ground two levels coarser, for cliffs.
fn coarse_at(g: Ground) -> vec4f {
    if (g.page < 0) {
        return vec4f(0.0, 0.0, 1.0, 1.0);
    }
    let r = coarse_read(g.level, g.page, g.base, g.p);
    let m = r.xy * 2.0 - 1.0;
    return vec4f(render_normal(vec3f(m, sqrt(max(1.0 - dot(m, m), 1e-4)))), 0.6 + 0.55 * r.z);
}

// Voronoi cells about `size` metres across: the nearest cell's hash, the gap to the next cell's edge in
// cell units, and a second hash for its shade.
fn voronoi(p: vec2f, size: f32) -> vec3f {
    let q = p / size;
    let i = floor(q);
    let f = q - i;
    var d1 = 8.0;
    var d2 = 8.0;
    var id = i;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2f(f32(x), f32(y));
            let d = length(o + vec2f(hash(i + o), hash(i + o + 17.0)) - f);
            if (d < d1) {
                d2 = d1;
                d1 = d;
                id = i + o;
            } else if (d < d2) {
                d2 = d;
            }
        }
    }
    return vec3f(hash(id + 5.0), d2 - d1, hash(id + 9.0));
}

// A patch of `colour` (alpha its share) over the share `under` of the ground, `r` metres from the eye,
// where the normal leans `north`.
// Dots: bushes `shape.x` metres across where a pixel can show them, their share of colour beyond.
// Else blotches about `shape.x` and 5.75 × `shape.x` metres across.
fn patched(albedo: vec3f, colour: vec4f, shape: vec2f, under: f32, world: vec2f, north: f32, r: f32, surface_size: f32) -> vec3f {
    if (colour.a <= 0.0 || under <= 0.0) {
        return albedo;
    }
    let size = shape.x;
    if (shape.y < 0.5) {
        let blotches = noise(world / size) * 0.55 + noise(world / (5.75 * size)) * 0.45;
        return mix(albedo, colour.rgb, under * smoothstep(1.0 - colour.a, 1.1 - colour.a, blotches));
    }
    // Slopes facing north stay moister and carry more scrub than slopes facing the sun.
    let cover = colour.a * under * (0.6 + 0.8 * noise(world / 90.0)) * (1.0 + clamp(3.0 * north, -0.3, 0.6)) * smoothstep(40.0, 70.0, r);
    let dotted = 1.0 - smoothstep(0.03 * size, 0.1 * size, surface_size);
    var dots = cover;
    if (DETAIL && dotted > 0.0) {
        let bush = voronoi(world, size);
        dots = mix(cover, step(bush.x, min(cover * 1.6, 1.0)) * smoothstep(0.18, 0.3, bush.y), dotted);
    }
    return mix(albedo, colour.rgb, 0.85 * dots);
}

// The plants of a row take ROW_SHARE of its pitch across, and darken the ground by ROW_DARK of their share.
const ROW_SHARE: f32 = 0.5;
const ROW_DARK: f32 = 0.7;

// The share of a pixel `w` rows wide about x that plants `share` of a row wide cover, a plant at each
// whole row plus 0.5: by the integral of the rows, and their mean where a pixel spans half a row.
fn row_cover(x: f32, share: f32, w: f32) -> f32 {
    let half = 0.5 * max(w, 1e-3);
    let cover = (row_integral(x + half, share) - row_integral(x - half, share)) / (2.0 * half);
    return mix(cover, share, smoothstep(0.25, 0.5, w));
}

fn row_integral(x: f32, share: f32) -> f32 {
    return floor(x) * share + clamp(fract(x) - 0.5 + 0.5 * share, 0.0, share);
}

// Blades of grass: BLADE_WIDTH wide and BLADE_LENGTH long in metres.
const BLADE_WIDTH: f32 = 0.025;
const BLADE_LENGTH: f32 = 0.3;

// Rows of blades along the direction `angle`, fixed to the ground; each blade tapers to a lighter tip.
fn blade_rows(world: vec2f, angle: f32) -> f32 {
    let d = vec2f(cos(angle), sin(angle));
    let across = dot(world, vec2f(-d.y, d.x)) / BLADE_WIDTH;
    let row = floor(across);
    let along = dot(world, d) / BLADE_LENGTH + hash(vec2f(row, 3.0));
    let shape = 1.0 - smoothstep(0.35 - 0.3 * fract(along), 0.5 - 0.3 * fract(along), abs(fract(across) - 0.5));
    return mix(-0.25, hash(vec2f(row, floor(along))) - 0.5 + 0.5 * (fract(along) - 0.5), shape);
}

// Value noise and its slope along x and y, per unit of q.
fn noised(q: vec2f) -> vec3f {
    let i = floor(q);
    let f = q - i;
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let a = hash(i);
    let b = hash(i + vec2f(1.0, 0.0));
    let c = hash(i + vec2f(0.0, 1.0));
    let d = hash(i + vec2f(1.0));
    return vec3f(a + (b - a) * u.x + (c - a) * u.y + (a - b - c + d) * u.x * u.y, du * (vec2f(b - a, c - a) + (a - b - c + d) * u.yx));
}

// Crags about `size` metres across, as smooth fields in the plane of the face, so a near wall shows no
// cells: how far a crag faces across the face and up it, each 0 to 1, and its joint, 0 on the joint line.
fn crag(p: vec2f, size: f32) -> vec3f {
    let q = p / size;
    let r = q * 2.3 + 17.0;
    // The steep middle of the smooth step gives each crag a face with a soft edge.
    let facing = 0.7 * smoothstep(vec2f(0.3), vec2f(0.7), vec2f(noise(q), noise(q + 41.0))) + 0.3 * vec2f(noise(r), noise(r + 23.0));
    return vec3f(facing, abs(2.0 * noise(q * 1.4 + 7.0) - 1.0));
}

// A road's cut or fill shows its detail this far from the eye: each pixel of it pays for two noises.
const CUT_DETAIL_M: f32 = 90.0;

// A road's cut or fill at `q` metres in the plane of its face, seen by a pixel `w` metres wide: tufts of the
// `ground` and bare `earth`, longer along the face than up it. The grain fades to its mean once a pixel spans it.
fn cut_face(ground: vec3f, earth: vec3f, q: vec2f, w: f32) -> vec3f {
    let p = q * vec2f(1.0, 1.8);
    let tuft = noise(p / 0.8);
    let grain = mix(0.5, noise(p / 0.17), 1.0 - smoothstep(0.05, 0.25, w));
    return mix(ground * (0.86 + 0.28 * tuft), mix(earth, ground, 0.45) * (0.8 + 0.4 * tuft), smoothstep(0.56, 0.64, tuft)) * (0.82 + 0.36 * grain);
}

// A distinct colour for each code, by the golden angle round the hue circle.
fn key(code: u32) -> vec3f {
    let h = fract(f32(code) * 0.618034);
    return 0.15 + 0.75 * clamp(abs((h * 6.0 + vec3f(0.0, 4.0, 2.0)) % 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

// The debug views of the ground, in sRGB: 5 material, 6 lithology, 7 region, 8 snow (white over blue-grey).
fn ground_debug(view: u32, level: u32, s: vec2f, g: Ground) -> vec3f {
    let k = min(level, look.codes.x);
    let t = texel_of(s, k);
    let holder = t.whole + vec2i(t.part >= vec2f(0.5));
    let page = ground_page(k, holder);
    if (page < 0) {
        return vec3f(0.45, 0.45, 0.6);
    }
    let cell = textureLoad(cells, in_page(page, holder & vec2i(255)), page_layer(page), 0);
    if (view == 5u) {
        return key(cell.r);
    }
    if (view == 6u) {
        return select(key(cell.g), vec3f(0.45, 0.45, 0.6), cell.g == 0u);
    }
    if (view == 7u) {
        return key(cell.b);
    }
    return mix(vec3f(0.45, 0.45, 0.6), vec3f(1.0), g.snow);
}

// The colour of the ground at a point: `s` in LOD space, `relative` in render space about the eye,
// `altitude` above sea level, on a node of `level`, in linear light with the haze, at the screen's `pixel`.
// The sizes of a pixel are those of an output pixel, so the detail follows the canvas at any render scale.
// `bench` is 1 on a path's band, which reads no mask. `d` holds the screen derivatives of `relative`, which the caller reads
// outside any branch that differs by the pixel.
fn ground_colour(s: vec2f, relative: vec3f, d: mat2x3f, altitude: f32, level: u32, pixel: vec2f, bench: f32) -> vec3f {
    let world = look.place.xy + relative.xy;
    let scale = look.screen.z;
    let dx = d[0].xy * scale;
    let dy = d[1].xy * scale;
    let footprint = max(length(dx), length(dy));
    let lateral = min(length(dx), length(dy));
    // The pixel's size on the surface itself: on a slope facing the eye it is far larger than its map
    // footprint, and texture finer than it shows as blotches.
    let surface_size = max(length(d[0]), length(d[1])) * scale;
    var facet = normalize(cross(d[0], d[1]));
    facet *= sign(facet.z);
    let drawn = drawn_normal(s);
    var n = render_normal(drawn);
    // Where the mesh slopes far more steeply than the ground's normals, it is a slope the mesh
    // smoothed out of detail finer than its triangles: it is cliff, lit like the cliff beside it.
    var skirt = select(smoothstep(0.25, 0.5, n.z - facet.z), 0.0, BAND);
    let sloped = n.z < 0.95 || skirt > 0.0;
    let g = ground_at(level, s, world, footprint);
    var open = g.open;
    let region = g.region;
    // The road paint first, so that only the Road stays live through the rest.
    var road = Road(vec3f(0.0), 0.0, 0.0, 0.0, 1.0, 1.0 - bench, bench);
    if (!BAND) {
        road = road_at(s, mat2x2f(dpdx(s), dpdy(s)) * scale, world, footprint, length(relative.xy), region, pixel, facet.z);
    }
    // A road's own facets are no cliff: the mesh cuts across the creases of its cut or fill.
    if (skirt > 0.0) {
        skirt *= 1.0 - road.shaped;
    }
    // A road's retaining wall, but for a cut in bare rock, and in woodland, as rock walls.
    let masoned = road.wall * (1.0 - road.cover) * (1.0 - g.barren) * (1.0 - g.tree);
    // The masonry runs along the world axis nearer to the wall, cross-faded within about 6° of 45°. By
    // the fine normal: the coarse one that a cliff takes below does not see a road's wall.
    let east = smoothstep(-0.1, 0.1, (abs(n.y) - abs(n.x)) / (abs(n.x) + abs(n.y) + 1e-6));
    // A cliff spans few texels across and many metres up, so its fine slopes would streak down the
    // face; it takes its normal from coarser levels instead.
    let cliff = 1.0 - smoothstep(0.4, 0.7, n.z);
    // Within a road's reach a slope past the 40° of its cut and fill is one of them, and its plan's texels stretch up it.
    var face = 0.0;
    // How steep the land is a few texels across: a road cutting or a bank is steep only at the finest
    // level, and stays vegetated.
    var broad = 0.0;
    // A path's bench is flatter than its hillside, and lit as itself would show as a pale strip beside the tread.
    if (sloped || road.bench > 0.0) {
        face = road.cuts * (1.0 - smoothstep(0.62, 0.84, n.z));
        let coarse = coarse_at(g);
        broad = max(1.0 - smoothstep(0.85, 0.95, coarse.z), skirt);
        // A slope that a road shaped is a cut or a fill: it stays as the satellite sees it, rock only
        // where that is bare.
        broad *= 1.0 - road.shaped * (1.0 - g.barren);
        let towards = max(max(cliff, skirt), max(face, road.bench));
        n = normalize(mix(n, coarse.xyz, towards));
        open = mix(open, coarse.w, towards);
    }
    let banded = g.banded;
    let slope = n.z;
    // The ground is mottled at every scale a pixel can show: patches tens of metres across, tussocks
    // and bare patches, and near the eye the sward's blades. Each scale fades to its mean once a pixel spans it.
    let r = length(relative.xy);
    // The share of the sward in tussocks and low shrubs that its material gives. Within about 50 m the
    // plant models stand instead.
    var clumped = 0.0;
    if (g.blades > 0.0 && r > 25.0 && road.cover < 1.0) {
        clumped = rule(region, slot_traits() + i32(g.material)).y * g.blades * (1.0 - road.cover) * smoothstep(25.0, 50.0, r);
    }
    var n15 = 0.5;
    var n25 = 0.5;
    var n08 = 0.5;
    if (footprint < 16.0) {
        n15 = noise(world / 15.0);
    }
    if (footprint < select(2.0, 6.0, clumped > 0.0)) {
        n25 = noise(world / 2.5);
    }
    if (footprint < 2.0) {
        n08 = noise(world / 0.8);
    }
    let mottle = 0.14 * (n15 - 0.5) * (1.0 - smoothstep(4.0, 16.0, footprint)) + (0.22 * n25 + 0.12 * n08 - 0.17) * (1.0 - smoothstep(0.3, 2.0, footprint));
    var albedo = g.albedo * (1.0 + mottle);
    if (face > 0.0 && r < CUT_DETAIL_M) {
        var earth = albedo;
        // Grass holds a bank up to about 55°; past that it stands bare, as rock where the ground is barren.
        if (n.z < 0.62) {
            let bare = mix(rule(region, slot_roads() + i32(CODE_DIRT) - 1).rgb, rule(region, slot_lithologies() + i32(g.lithology)).rgb, g.barren);
            earth = mix(bare, albedo, smoothstep(0.45, 0.62, n.z));
        }
        let cut = cut_face(albedo, earth, vec2f(select(world.x, world.y, abs(n.x) > abs(n.y)), altitude), surface_size);
        albedo = mix(albedo, cut, face * (1.0 - smoothstep(0.6 * CUT_DETAIL_M, CUT_DETAIL_M, r)));
    }
    // Tussocks and low shrubs, more in some stretches about 60 m across than in others: tufts about 1 m,
    // shrubs about 3 m and thickets about 15 m across on the mottle's highs, and bushes 1 to 5 m long,
    // each darker and more olive than the sward. Each fades to its mean once a pixel spans it. The ground
    // between them is lighter, so the sward keeps its mean colour.
    if (clumped > 0.0) {
        clumped = min(clumped * (0.5 + noise(world / 60.0 + 23.0)), 0.75);
        // Value noise lies above this on about `clumped` of the ground.
        let above = 0.5 + (0.5 - clumped) / 1.65;
        let tuft = mix(smoothstep(above, above + 0.06, n08), clumped, smoothstep(0.3, 1.0, footprint));
        let shrub = mix(smoothstep(above + 0.04, above + 0.1, n25), 0.85 * clumped, smoothstep(1.5, 6.0, footprint));
        let thicket = mix(smoothstep(above + 0.08, above + 0.14, n15), 0.7 * clumped, smoothstep(4.0, 16.0, footprint));
        // A bush covers 0.1 of its cell on average, and stands in about 0.6 × clumped of the cells.
        let area = 0.06 * clumped;
        var bush = area;
        if (DETAIL && footprint < 2.5) {
            // At most one bush in each 8 m cell, its middle inside its cell, so no neighbour is read. The
            // cells are turned and warped by the mottle's noise, so the bushes lie off any grid with ragged
            // outlines; the 15 m noise gathers them in groups. A bush is longer along the contour.
            let turn = mat2x2f(0.8, 0.6, -0.6, 0.8);
            let q = turn * world + 6.0 * (n15 - 0.5) * vec2f(1.0, 0.6) + 3.0 * (n25 - 0.5) * vec2f(-0.5, 1.0);
            let cell = floor(q / 8.0);
            let h = hash(cell + 41.0);
            let fade = smoothstep(0.5, 2.5, footprint);
            bush = area * fade;
            if (fract(h * 3.71) < 1.2 * clumped * smoothstep(0.35, 0.65, n15)) {
                let d = q - (cell + 0.3 + 0.4 * fract(h * vec2f(7.13, 13.7))) * 8.0;
                let size = fract(h * 31.3);
                let radius = (0.5 + 1.9 * size * size) * (0.75 + 0.5 * n08);
                let along = normalize(turn * vec2f(-n.y, n.x) * 3.0 + fract(h * vec2f(5.17, 2.93)) - 0.5);
                let dist = length(vec2f(dot(d, along) / (1.0 + 0.6 * fract(h * 11.3)), dot(d, vec2f(-along.y, along.x))));
                bush = mix((0.75 + 0.5 * fract(h * 17.9)) * (1.0 - smoothstep(radius - 0.3, radius + 0.3, dist)), area, fade);
            }
        }
        let dark = min(0.45 * tuft + 0.45 * shrub + 0.35 * thicket, 0.85);
        albedo *= mix(vec3f(1.0), vec3f(0.42, 0.5, 0.35), dark) * mix(1.0, 0.45, bush) / ((1.0 - 0.56 * clumped) * (1.0 - 0.55 * area));
    }
    // Blades: rows of strokes fixed to the ground, so they stay in place when the eye moves. A pixel
    // takes the rows of the one or two of 32 directions nearest its line from the eye, so a blade
    // stands upright on the screen. Only near the eye, on ground it looks along: a bank that faces it
    // would stretch them tall.
    if (DETAIL && r < 45.0) {
        let blades = (1.0 - smoothstep(0.4, 0.8, lateral / BLADE_WIDTH)) * (1.0 - smoothstep(0.25, 0.6, footprint / BLADE_LENGTH))
            * (1.0 - smoothstep(25.0, 45.0, r)) * (1.0 - smoothstep(0.45, 0.75, dot(n, normalize(-relative)))) * g.blades;
        if (blades > 0.0) {
            let sector = atan2(relative.y, relative.x) / 6.2831853 * 32.0;
            let first = floor(sector);
            let w = smoothstep(0.35, 0.65, sector - first);
            var blade = 0.0;
            if (w < 1.0) {
                blade = blade_rows(world, first * 6.2831853 / 32.0);
            }
            if (w > 0.0) {
                blade = mix(blade, blade_rows(world, (first + 1.0) * 6.2831853 / 32.0), w);
            }
            albedo *= 1.0 + 0.3 * blade * blades;
        }
    }
    // The materials planted in rows paint the rows of their fields (fields.wgsl), where a pixel can show
    // them: darker where the plants stand than in the alleys, so the ground keeps its mean.
    let rows = rule(region, slot_rows() + i32(g.material));
    if (rows.y > 2.0 * lateral && road.cover < 1.0) {
        let u = units_of(s);
        let field = field_of(field_cells(u), u);
        let f = field_rows(field.cell, rows);
        let d = (u - vec2f(field_site(field.cell) - surface.anchor)) * f.unit;
        let across = vec2f(-f.along.y, f.along.x);
        // The render pixel's steps in world axes, y south, and its width in rows along and across.
        let ex = vec2f(dx.x, -dx.y) / scale;
        let ey = vec2f(dy.x, -dy.y) / scale;
        let w = vec2f(abs(dot(ex, f.along)) + abs(dot(ey, f.along)), abs(dot(ex, across)) + abs(dot(ey, across))) / f.pitch;
        let p = vec2f(dot(d, f.along), dot(d, across)) / f.pitch;
        // A plant covers ROW_SHARE of the pitch across the rows and as many metres along them, all of a hedge.
        let hedge = rule(region, slot_traits() + i32(g.material)).z > 0.5;
        let lengthwise = select(min(1.0, ROW_SHARE * f.pitch.y / f.pitch.x), 1.0, hedge);
        // The headland is bare, and the rows' mean where a pixel spans half a row, as the rows are.
        let headland = clamp((field.edge * f.unit - HEADLAND_M) / max(lateral, 0.1) + 0.5, 0.0, 1.0);
        let inside = mix(headland, 1.0, smoothstep(0.25, 0.5, max(w.x, w.y)));
        let planted = row_cover(p.y, ROW_SHARE, w.y) * row_cover(p.x, lengthwise, w.x) * inside;
        albedo *= 1.0 + ROW_DARK * (ROW_SHARE * lengthwise - planted) * (1.0 - road.cover);
    }
    // The patches of the region in their order: per patch its size and 1 for dots.
    let shapes = rule(region, slot_cliff() + 8);
    albedo = patched(albedo, rule(region, slot_cliff() + 1), shapes.xy, g.patches.x, world, n.y, r, surface_size);
    albedo = patched(albedo, rule(region, slot_cliff() + 2), shapes.zw, g.patches.y, world, n.y, r, surface_size);
    // The strewn rocks that their instances no longer show (rocks.wgsl).
    let far = length(relative);
    let end = ROCK_REACH_M * look.screen.w;
    if (far > (1.0 - ROCK_HANDOFF) * end) {
        let rise = sqrt(max(1.0 - drawn.z * drawn.z, 0.0)) / max(drawn.z, 0.1);
        albedo = mix(albedo, rock_seen(relative / far, n), rocks_painted(s, vec4u(g.material, g.lithology, region, 0u), rise, far, end, footprint));
    }
    albedo = mix(albedo, road.colour, road.cover * (1.0 - road.share));
    if (DETAIL && masoned > 0.0) {
        let stone = rule(region, slot_lithologies() + i32(g.lithology)).rgb;
        // A coordinate along the wall itself would turn with its normal: far from the frame's origin, a
        // slight turn is a large step.
        var blocks = vec3f(0.0);
        if (east < 1.0) {
            blocks = masonry(stone, world.y, altitude, surface_size);
        }
        if (east > 0.0) {
            blocks = mix(blocks, masonry(stone, world.x, altitude, surface_size), east);
        }
        albedo = mix(albedo, blocks, masoned);
    }
    let bare_from = g.bare_from;
    // Rock breaks into crags about 26 m across, in the plane of the face.
    // A crag that faces up holds turf where the satellite sees green.
    let rock = skirt > 0.0 || (broad > 0.0 && slope < mix(bare_from + 0.12, 0.95, banded));
    let on_face = vec2f(select(world.x, world.y, abs(n.x) > abs(n.y)), altitude);
    let big = select(0.0, 1.0 - smoothstep(4.0, 12.0, surface_size), rock);
    // Gullies and buttresses tens of metres wide and a hundred tall bend the crags.
    var buttress = vec3f(0.5, 0.0, 0.0);
    if (rock && surface_size < 25.0) {
        buttress = noised(on_face / vec2f(60.0, 150.0));
    }
    let bent = on_face + 10.0 * buttress.yz;
    var crags = vec3f(0.5, 0.5, 1.0);
    if (big > 0.0) {
        crags = crag(mat2x2f(0.87, 0.5, -0.5, 0.87) * bent, 26.0);
    }
    let top = smoothstep(0.65, 0.85, crags.y) * big;
    let stepped = slope + 0.07 * (top - 0.25 * big);
    let steep = max((1.0 - smoothstep(mix(bare_from, 0.8, banded), mix(bare_from + 0.043, 0.88, banded), stepped)) * broad, skirt);
    let view = look.modes.x;
    if (view == 9u) {
        // The albedo without light: a seam in it is a seam of the ground pages.
        return pow(min(g.albedo * 3.0, vec3f(1.0)), vec3f(1.0 / 2.2));
    }
    if (view >= 5u) {
        return ground_debug(view, level, s, g) * (0.45 + 0.55 * max(dot(n, vec3f(-0.4, 0.4, 0.82)), 0.0));
    }
    let walled = steep * (1.0 - g.tree) * (1.0 - road.cover) * (1.0 - masoned);
    if (walled > 0.0) {
        // Variegated clays erode into badlands, so their beds show on gentler slopes: red, white and
        // green-grey, streaked by gullies.
        var stone = rule(region, slot_lithologies() + i32(g.lithology)).rgb;
        if (banded > 0.0) {
            let across = (world.x - world.y) * 0.7071;
            let layer = 0.7 * noise(vec2f(altitude / 4.0, across / 400.0)) + 0.3 * noise(vec2f(across / 2.5, altitude / 30.0));
            let beds = mix(mix(look.variegated[0].rgb, look.variegated[1].rgb, smoothstep(0.48, 0.53, layer)), look.variegated[2].rgb, smoothstep(0.58, 0.63, layer));
            stone = mix(stone, beds, banded);
        }
        let wall = rule(region, slot_cliff());
        var face = mix(mix(stone, wall.rgb, wall.a), g.albedo, g.barren * smoothstep(0.68, 0.8, n.z));
        // Lit and weathered stretches of the wall, tens of metres across.
        let stain = noise(world / 70.0 + altitude / 50.0);
        // Each crag faces a little across the face and up or down it; the gullies and buttresses tilt
        // the face itself.
        let sheer = 1.0 - smoothstep(0.3, 0.55, n.z);
        let across = normalize(vec3f(-n.y, n.x, 0.0) + 1e-6);
        let up = cross(n, across);
        var tilt = (crags.xy - 0.5) * 1.3 * big;
        var gully = 0.5;
        var gully_tilt = 0.0;
        if (sheer > 0.0 && surface_size < 25.0) {
            let wide = 1.0 - smoothstep(8.0, 25.0, surface_size);
            gully = mix(0.5, buttress.x, wide);
            gully_tilt = -0.8 * buttress.y * wide * sheer;
        }
        let plain = n;
        n = normalize(n + across * gully_tilt * walled);
        // A facet's light rises and falls as far as it turns toward the sun that reaches it, and in shade
        // toward the sun's side of the sky and up; a gully sees less of the sky. Close by, the crags differ less,
        // so a near wall reads as one rough surface.
        let calm = 1.0 - smoothstep(1.0, 3.0, surface_size);
        tilt *= 1.0 - 0.35 * calm;
        let turned = across * tilt.x + up * tilt.y;
        let reach = smoothstep(-0.2, 0.1, dot(plain, look.sun.xyz));
        face *= 1.0 + 0.9 * dot(turned, look.sun.xyz) * reach * g.sun * walled;
        open *= mix(1.0, (1.0 + 0.6 * tilt.x * dot(across.xy, normalize(look.sun.xy + 1e-6)) + 0.3 * tilt.y) * mix(0.75, 1.25, gully), walled);
        let contrast = 1.0 - 0.5 * calm;
        let bulge = mix(1.0, mix(0.81, 1.05, smoothstep(0.0, 0.2, crags.z)), big * contrast);
        var tone = 1.0 + 0.4 * (crags.x - crags.y) * big * contrast;
        // The bedding shows only as a faint tone that comes and goes.
        let bed = hash(vec2f(floor(dot(vec3f(world, altitude), rule(region, slot_cliff() + 4).xyz) / 11.0), 3.0));
        tone *= 1.0 + 0.08 * (bed - 0.5) * (1.0 - smoothstep(0.4, 0.6, stain)) * (1.0 - smoothstep(3.0, 10.0, surface_size));
        face *= tone * bulge * (0.85 + 0.3 * stain) * mix(0.75, 1.25, gully) * (1.0 + 0.04 * sheer);
        face = mix(face, albedo, top * smoothstep(0.35, 0.6, plain.z) * g.green);
        if (altitude < 16.0) {
            face *= 1.0 - 0.45 * (1.0 - smoothstep(1.0, 4.0, altitude));
            face = mix(face, vec3f(0.42, 0.2, 0.06), 0.28 * smoothstep(3.0, 6.0, altitude) * (1.0 - smoothstep(10.0, 16.0, altitude))
                * smoothstep(0.45, 0.7, noise(world / 3.0 + altitude / 2.0)));
        }
        albedo = mix(albedo, face, walled);
    }
    // The snow under the trees stays at every distance; past their reach the crowns that no longer stand
    // hide a share of it. Snow slides off walls, which the data's texels are too wide to see.
    let crowns = g.tree * CROWN_SHARE * smoothstep(0.8 * TREE_REACH_M, TREE_REACH_M, length(relative.xy));
    let snow = g.snow * (1.0 - crowns) * smoothstep(0.4, 0.6, n.z);
    albedo = mix(albedo, rule(region, i32(COVER_SNOW) * 3).rgb, snow) * (1.0 - 0.3 * look.light.w * (1.0 - snow));
    // Rock shows its relief in the smooth light: the bands would light every face toward the sun alike.
    let c = light_up(albedo, n, sun_share(g.sun, relative, n, LIFT_GROUND), open, mix(look.horizon.w, 0.5, walled));
    return atmosphere(c, relative, altitude);
}
