// The ground look: a port of the ground shading of the prototype (`groundOn`, `groundAt` and
// `terrainFragment` of src/shaders.ts) over the ground pages. A node draws the ground of its level.
// What the prototype measured from its fixed spot, this measures from the eye. Terrain shadows, clouds, roads
// and water are not here yet.
// #include ../terrain/drawn-height.wgsl
// #include ../light/shade.wgsl

// Per ground node MAP × MAP entries per level about the eye, in the windows of the LOD map: the page + 1, or 0.
@group(2) @binding(1) var<storage, read> ground_map: array<u32>;
// Per texel: the material, the lithology, the region and barren × 255.
@group(2) @binding(2) var cells: texture_2d_array<u32>;
// Per texel: the satellite on the date, and its NDVI code (0 where no quarter looked).
@group(2) @binding(3) var colour: texture_2d_array<f32>;
// Per texel: snow by the region's snow_share, snow data known, the satellite's snow mark, the share of seasons with snow.
@group(2) @binding(4) var snow_state: texture_2d_array<f32>;
// Per texel: how open the ground is, (open − 0.6) / 0.55.
@group(2) @binding(5) var openness: texture_2d_array<f32>;
// Per region a row of slots: ground/rules.ts.
@group(2) @binding(6) var rules: texture_2d<f32>;
@group(2) @binding(7) var filtered: sampler;
// The ground lab `cache`: per texel four layers of ground parameters, ground.wgsl.
@group(2) @binding(8) var cache: texture_2d_array<f32>;

const COVER_TREE: u32 = 0u;
const COVER_SHRUB: u32 = 1u;
const COVER_GRASS: u32 = 2u;
const COVER_BUILT: u32 = 4u;
const COVER_BARE: u32 = 5u;
const COVER_SNOW: u32 = 6u;
const COVER_WATER: u32 = 7u;
const COVER_WETLAND: u32 = 8u;
const COVER_MOSS: u32 = 10u;

// Pages per layer side of the ground textures.
const GROUND_SIDE: i32 = 4;

struct Ground {
    albedo: vec3f,
    water: f32,
    tree: f32,
    // The satellite's snow mark, then the snow on the date.
    snow: f32,
    // Snow data: snow by the share rule, known, and the share × known.
    data: vec3f,
    // Open, unfarmed ground; how lush it is, 0 where no satellite pixel says; heath and bog; exposed rock and scree.
    rough: f32,
    lush: f32,
    moor: f32,
    bare: f32,
    // Land that no quarter sees green; ground that grows a sward.
    barren: f32,
    sward: f32,
    // The rock: its colour, its banded share and how hard it stands.
    stone: vec3f,
    banded: f32,
    hardness: f32,
    open: f32,
    region: u32,
}

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.3, 0.59, 0.11));
}

// Dave Hoskins' hash and a smooth value noise over it.
fn hash(q: vec2f) -> f32 {
    var p3 = fract(q.xyx * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn noise(q: vec2f) -> f32 {
    let i = floor(q);
    var f = q - i;
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2f(1.0, 0.0)), f.x), mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0)), f.x), f.y);
}

fn rule(region: u32, slot: i32) -> vec4f {
    return textureLoad(rules, vec2i(slot, i32(region)), 0);
}

// The slots of ground/rules.ts, from the sizes of the vocabulary.
fn slot_materials() -> i32 { return 33; }
fn slot_satellite() -> i32 { return 33 + i32(look.codes.z); }
fn slot_lithologies() -> i32 { return 33 + 2 * i32(look.codes.z); }
fn slot_cliff() -> i32 { return slot_lithologies() + i32(look.codes.w); }
fn slot_kinds() -> i32 { return slot_cliff() + 15; }
fn slot_hardness() -> i32 { return slot_kinds() + i32(look.codes.z); }

// The cover's ramp in the region, low NDVI to high.
fn ramp(region: u32, cover: u32, t: f32) -> vec3f {
    let slot = i32(cover) * 3 + select(1, 0, t < 0.5);
    return mix(rule(region, slot).rgb, rule(region, slot + 1).rgb, select(2.0 * t - 1.0, 2.0 * t, t < 0.5));
}

// What one texel adds to the ground, through its region's rules, with weight w.
struct Parts {
    palette: vec3f,
    paint: vec4f,
    stone: vec3f,
    water: f32,
    tree: f32,
    rough: f32,
    moor: f32,
    sward: f32,
    bare: f32,
    banded: f32,
    hardness: f32,
}

fn add_texel(sum: ptr<function, Parts>, cell: vec4u, w: f32, t: f32, ledge: f32, barren: f32, decided: bool) {
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
    // Exposed rock where the satellite sees green is a ledge of turf: the region's heath where it paints one, else grass.
    let turf = kinds.z * ledge;
    if (turf > 0.0) {
        c = mix(c, ramp(own, COVER_GRASS, t), turf);
    }
    let named = rule(own, slot_materials() + i32(cell.r));
    let rock = rule(own, slot_lithologies() + i32(cell.g));
    (*sum).palette += w * c;
    (*sum).paint += w * named.a * (1.0 - turf) * vec4f(named.rgb, 1.0);
    if (turf > 0.0) {
        let heath = rule(own, slot_materials() + i32(look.codes.x));
        (*sum).paint += w * heath.a * turf * vec4f(heath.rgb, 1.0);
    }
    (*sum).water += select(0.0, w, cover == COVER_WATER);
    (*sum).tree += select(0.0, w, cover == COVER_TREE);
    (*sum).rough += w * kinds.a;
    (*sum).moor += w * kinds.y;
    (*sum).sward += select(w, 0.0, cover == COVER_BARE || cover == COVER_BUILT || cover == COVER_SNOW || cover == COVER_WATER || cover == COVER_MOSS);
    (*sum).bare += w * kinds.z;
    (*sum).stone += w * rock.rgb;
    (*sum).banded += w * rock.a;
    (*sum).hardness += w * rule(own, slot_hardness() + i32(cell.g)).x;
}

// The albedo of the parts, with the satellite's chroma rescaled to the palette's luma, bare_luma,
// and the paint of the nearest material.
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
        // The paint keeps its nearest material's share of the satellite's hue, at its own brightness.
        let keep = select(0.0, rule(region, slot_satellite() + i32(material)).x, parts.paint.a > 0.0);
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

fn in_page(page: i32, c: vec2i) -> vec2i {
    return clamp(c, vec2i(0), vec2i(255)) + 256 * vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE);
}

fn page_layer(page: i32) -> i32 {
    return page / (GROUND_SIDE * GROUND_SIDE);
}

// A filtered read at a point `local` texels from the page's corner, kept inside the page.
fn page_uv(page: i32, local: vec2f) -> vec2f {
    return (vec2f(256 * vec2i(page % GROUND_SIDE, (page / GROUND_SIDE) % GROUND_SIDE)) + clamp(local, vec2f(0.5), vec2f(255.5))) / f32(256 * GROUND_SIDE);
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

// A bilinear read of a page texture at point p of a level, in texels from the corner `base` of page
// `home` (texel centres are whole numbers), across the page borders: the sampler inside the page,
// four loads where the footprint crosses a border. A page has `planes` layers; this reads `plane`.
fn page_read(tex: texture_2d_array<f32>, level: u32, home: i32, base: vec2i, p: vec2f, planes: i32, plane: i32) -> vec4f {
    let c = vec2i(floor(p));
    if (all(c >= vec2i(0)) && all(c < vec2i(255))) {
        return textureSampleLevel(tex, filtered, page_uv(home, p + 0.5), page_layer(home) * planes + plane, 0.0);
    }
    let f = p - floor(p);
    var out = vec4f(0.0);
    for (var j = 0; j < 4; j++) {
        let o = vec2i(j & 1, j >> 1);
        let at = texel_at(level, home, base, c + o);
        out += select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1) * textureLoad(tex, at.xy, at.z * planes + plane, 0);
    }
    return out;
}

// The ground at a point of LOD space on a level, from the page of that level or the next coarser one
// that is resident. `world` is the point in the frame, for the brush of the texel edges.
fn ground_on(level: u32, s: vec2f, world: vec2f) -> Ground {
    var k = level;
    var t = texel_of(s, k);
    var page = -1;
    loop {
        t = texel_of(s, k);
        page = ground_page(k, t.whole + vec2i(t.part >= vec2f(0.5)));
        if (page >= 0 || k <= COARSEST) {
            break;
        }
        k--;
    }
    var g: Ground;
    if (page < 0) {
        // No level has ground here yet: the palette's grass of the first region.
        g.albedo = ramp(0u, COVER_GRASS, 0.5);
        g.open = 1.0;
        return g;
    }
    // Texel coordinates from the page's corner: texel centres are whole numbers.
    let base = (t.whole + vec2i(t.part >= vec2f(0.5))) & vec2i(~255);
    let texel = vec2f(t.whole - base) + t.part;
    let sky_seen = page_read(colour, k, page, base, texel, 1, 0);
    let data = page_read(snow_state, k, page, base, texel, 1, 0);
    g.open = 0.6 + 0.55 * page_read(openness, k, page, base, texel, 1, 0).r;
    g.snow = data.b;
    g.data = vec3f(data.r, data.g, data.a);
    let sat = satellite(sky_seen);
    let ndvi = ndvi_of(sky_seen);
    let t_ndvi = clamp((ndvi - 0.15) / 0.6, 0.0, 1.0);
    g.lush = select(0.0, t_ndvi, sat.a > 0.0);
    // A wobble of up to half a texel turns the texel stairs into brush edges.
    let q = world / (surface.unit * f32(1 << (22u - k))) * 0.7;
    let brushed = texel + 0.45 * (vec2f(noise(q), noise(q + 31.0)) * 2.0 - 1.0);
    if (look.modes.y == 1u) {
        // The parameter cache: the same fields per texel, filtered, at the brushed point.
        let p0 = page_read(cache, k, page, base, brushed, 4, 0);
        let p1 = page_read(cache, k, page, base, brushed, 4, 1);
        let p2 = page_read(cache, k, page, base, brushed, 4, 2);
        let p3 = page_read(cache, k, page, base, brushed, 4, 3);
        g.albedo = p0.rgb * p0.rgb;
        g.water = p0.a;
        g.stone = p1.rgb * p1.rgb;
        g.banded = p1.a;
        g.tree = p2.r;
        g.rough = p2.g;
        g.moor = p2.b;
        g.bare = p2.a;
        g.barren = p3.r;
        g.sward = p3.g;
        g.hardness = p3.a;
        g.region = textureLoad(cells, in_page(page, vec2i(floor(texel + 0.5))), page_layer(page), 0).b;
        return g;
    }
    // The four texels around the brushed point, each its material through its region's rules,
    // blended as a bilinear sample would be.
    let corner = vec2i(floor(brushed));
    let f = brushed - floor(brushed);
    var taps: array<vec4u, 4>;
    var barren = 0.0;
    for (var j = 0; j < 4; j++) {
        let o = vec2i(j & 1, j >> 1);
        let at = texel_at(k, page, base, corner + o);
        taps[j] = textureLoad(cells, at.xy, at.z, 0);
        barren += select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1) * f32(taps[j].a) / 255.0;
    }
    // Most texels lie among their like: then one corner holds them all.
    let alike = all(taps[1].rgb == taps[0].rgb) && all(taps[2].rgb == taps[0].rgb) && all(taps[3].rgb == taps[0].rgb);
    let ledge = smoothstep(0.35, 0.6, ndvi);
    let decided = data.g > 0.5 || sat.a > 0.0;
    var parts: Parts;
    var nearest = 0.0;
    var material = 0u;
    for (var j = 0; j < select(4, 1, alike); j++) {
        let o = vec2i(j & 1, j >> 1);
        let w = select(select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1), 1.0, alike);
        add_texel(&parts, taps[j], w, t_ndvi, ledge, barren, decided);
        if (w > nearest) {
            nearest = w;
            g.region = taps[j].b;
            material = taps[j].r;
        }
    }
    g.albedo = albedo_of(parts, g.region, material, sat, barren);
    g.water = parts.water;
    g.tree = parts.tree;
    g.rough = parts.rough;
    g.moor = parts.moor;
    g.bare = parts.bare;
    g.barren = barren * (1.0 - parts.water);
    g.sward = parts.sward;
    g.stone = parts.stone;
    g.banded = parts.banded;
    g.hardness = parts.hardness;
    return g;
}

// How open the ground is at a point on a level, or on the next coarser level that is resident.
fn open_on(level: u32, s: vec2f) -> f32 {
    for (var k = level; k >= COARSEST; k--) {
        let t = texel_of(s, k);
        let holder = t.whole + vec2i(t.part >= vec2f(0.5));
        let page = ground_page(k, holder);
        if (page >= 0) {
            let base = holder & vec2i(~255);
            return 0.6 + 0.55 * page_read(openness, k, page, base, vec2f(t.whole - base) + t.part, 1, 0).r;
        }
    }
    return 1.0;
}

// The ground under a point of a node of `level`, from its level's page. The satellite decides snow
// only where the data has none, and a snow edge is crisper than the texels' blend across it.
fn ground_at(level: u32, s: vec2f, world: vec2f) -> Ground {
    var g = ground_on(min(level, look.codes.y), s, world);
    g.snow = smoothstep(0.25, 0.75, g.data.x + (1.0 - g.data.y) * g.snow) * (1.0 - g.water);
    return g;
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

// Crags about `size` metres across: the nearest crag's two hashes and the gap to its edge, in crag
// units. Each crag's point keeps to the middle of its cell, so the nearest lies among the four cells around p.
fn crag(p: vec2f, size: f32) -> vec3f {
    let q = p / size - 0.5;
    let i = floor(q);
    let f = q - i;
    var d1 = 8.0;
    var d2 = 8.0;
    var id = i;
    for (var j = 0; j < 4; j++) {
        let o = vec2f(f32(j & 1), f32(j >> 1));
        let d = length(o + 0.15 + 0.7 * vec2f(hash(i + o), hash(i + o + 17.0)) - f);
        if (d < d1) {
            d2 = d1;
            d1 = d;
            id = i + o;
        } else if (d < d2) {
            d2 = d;
        }
    }
    return vec3f(hash(id + 5.0), hash(id + 9.0), d2 - d1);
}

// A normal of LOD space axes in render space, with the linear part of the cubic.
fn render_normal(m: vec3f) -> vec3f {
    return normalize(vec3f(m.x * vec2f(coefficient(1u), coefficient(11u)) + m.y * vec2f(coefficient(2u), coefficient(12u)), m.z));
}

// The normal `up` levels coarser than the LOD, for cliffs: their fine slopes would streak down a face.
fn coarse_normal(s: vec2f, up: f32) -> vec3f {
    let level = u32(max(ceil(lod(s) - up), f32(COARSEST)));
    let n = level_normal(s, level);
    return render_normal(vec3f(n, sqrt(max(1.0 - dot(n, n), 1e-4))));
}

// A distinct colour for each code, by the golden angle round the hue circle.
fn key(code: u32) -> vec3f {
    let h = fract(f32(code) * 0.618034);
    return 0.15 + 0.75 * clamp(abs((h * 6.0 + vec3f(0.0, 4.0, 2.0)) % 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

// The debug views of the ground, in sRGB: 5 material, 6 lithology, 7 region, 8 snow (white, the
// share of past seasons with snow on the date red, no snow data blue-grey).
fn ground_debug(view: u32, level: u32, s: vec2f, g: Ground) -> vec3f {
    let k = min(level, look.codes.y);
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
    let share = select(-1.0, g.data.z / g.data.y, g.data.y > 0.0);
    return mix(select(vec3f(0.15 + 0.85 * share, 0.15, 0.15), vec3f(0.45, 0.45, 0.6), share < 0.0), vec3f(1.0), g.snow);
}

// The colour of the ground at a point: `s` in LOD space, `relative` in render space about the eye,
// `altitude` above sea level, on a node of `level`. Ends with the haze and `finish()`.
fn ground_colour(s: vec2f, relative: vec3f, altitude: f32, level: u32) -> vec3f {
    let world = look.place.xy + relative.xy;
    let footprint = max(length(dpdx(relative.xy)), length(dpdy(relative.xy)));
    // The pixel's size on the surface itself: on a slope facing the eye it is far larger than its map
    // footprint, and texture finer than it shows as blotches.
    let surface_size = max(length(dpdx(relative)), length(dpdy(relative)));
    var facet = normalize(cross(dpdx(relative), dpdy(relative)));
    facet *= sign(facet.z);
    var n = render_normal(drawn_normal(s));
    var g = ground_at(level, s, world);
    var open = g.open;
    let region = g.region;
    // outcrops, cos turf_deg, snow_share, speckle size
    let terrain = rule(region, slot_cliff() + 3);
    let bedding = rule(region, slot_cliff() + 4).xyz;
    // Where the mesh slopes far more steeply than the ground's normals, it is a slope the mesh
    // smoothed out of detail finer than its triangles: it is cliff, lit like the cliff beside it.
    let skirt = smoothstep(0.25, 0.5, n.z - facet.z);
    // A cliff spans few texels across and many metres up, so its fine slopes would streak down the
    // face; it takes its normal from coarser levels instead.
    let cliff = 1.0 - smoothstep(0.4, 0.7, n.z);
    // How steep the land is a few texels across: a road cutting or a bank is steep only at the finest
    // level, and stays vegetated.
    var broad = 0.0;
    if (n.z < 0.95 || skirt > 0.0) {
        let coarse = coarse_normal(s, 2.0);
        let coarse_open = open_on(u32(max(f32(min(level, look.codes.y)) - 2.0, f32(COARSEST))), s);
        broad = max(1.0 - smoothstep(0.85, 0.95, coarse.z), skirt);
        let towards = max(cliff, skirt);
        n = normalize(mix(n, coarse, towards));
        open = mix(open, coarse_open, towards);
    }
    // Steep open ground is rock; trees hold on to it. Variegated clays erode into badlands, so their
    // beds show on gentler slopes: red, white and green-grey, streaked by gullies.
    var stone = g.stone;
    let banded = g.banded;
    let hardness = g.hardness;
    if (banded > 0.0) {
        let across = (world.x - world.y) * 0.7071;
        let layer = 0.7 * noise(vec2f(altitude / 4.0, across / 400.0)) + 0.3 * noise(vec2f(across / 2.5, altitude / 30.0));
        let beds = mix(mix(look.variegated[0].rgb, look.variegated[1].rgb, smoothstep(0.48, 0.53, layer)), look.variegated[2].rgb, smoothstep(0.58, 0.63, layer));
        stone = mix(stone, beds, banded);
    }
    // Where the satellite sees green, a steep slope is turf and scrub on the rock up to the region's turf angle at least.
    let green = smoothstep(0.4, 0.7, g.lush) * (1.0 - g.bare);
    let slope = n.z;
    // The ground is mottled at every scale a pixel can show: patches tens of metres across, tussocks
    // and bare patches, and near the eye the sward's blades. Each scale fades to its mean once a pixel spans it.
    var mottle = 0.0;
    if (footprint < 16.0) {
        mottle = 0.14 * (noise(world / 15.0) - 0.5) * (1.0 - smoothstep(4.0, 16.0, footprint));
    }
    if (footprint < 2.0) {
        mottle += (0.22 * noise(world / 2.5) + 0.12 * noise(world / 0.8) - 0.17) * (1.0 - smoothstep(0.3, 2.0, footprint));
    }
    var albedo = g.albedo * (1.0 + mottle);
    // Blades: strokes that point away from the eye, so they stand upright on the screen. Each is a
    // fixed angle wide and a fixed share of its distance deep, and tapers to a lighter tip. Only near
    // the eye, on ground it looks along: a bank that faces it would stretch them tall.
    let r = max(length(relative.xy), 0.5);
    if (r < 45.0) {
        let blades = (1.0 - smoothstep(0.25, 0.6, footprint / (0.08 * r))) * (1.0 - smoothstep(25.0, 45.0, r))
            * (1.0 - smoothstep(0.45, 0.75, dot(n, normalize(-relative)))) * g.sward * (1.0 - g.barren);
        if (blades > 0.0) {
            let angle = (atan2(relative.y, relative.x) / 6.2831853 + 0.5) * 1600.0;
            let along = log(r) / 0.08 + hash(vec2f(floor(angle), 3.0));
            let shape = 1.0 - smoothstep(0.35 - 0.3 * fract(along), 0.5 - 0.3 * fract(along), abs(fract(angle) - 0.5));
            let blade = mix(-0.25, hash(vec2f(floor(angle), floor(along))) - 0.5 + 0.5 * (fract(along) - 0.5), shape);
            albedo *= 1.0 + 0.3 * blade * blades;
        }
    }
    // Scattered bushes on open ground: dots where a pixel can show them, their share of colour beyond.
    let speckle = rule(region, slot_cliff() + 2);
    if (speckle.a > 0.0) {
        // Slopes facing north stay moister and carry more scrub than slopes facing the sun.
        let cover = speckle.a * g.rough * (0.6 + 0.8 * noise(world / 90.0)) * (1.0 + clamp(3.0 * n.y, -0.3, 0.6)) * smoothstep(40.0, 70.0, length(relative.xy));
        let bush = voronoi(world, terrain.w);
        let dots = step(bush.x, min(cover * 1.6, 1.0)) * smoothstep(0.18, 0.3, bush.y);
        albedo = mix(albedo, speckle.rgb, 0.85 * mix(cover, dots, 1.0 - smoothstep(0.03 * terrain.w, 0.1 * terrain.w, surface_size)));
    }
    // Heather in patches over the moor.
    if (g.moor > 0.0) {
        let patches = noise(world / 4.0) * 0.55 + noise(world / 23.0) * 0.45;
        let heather = rule(region, slot_cliff() + 1);
        albedo = mix(albedo, heather.rgb, g.moor * smoothstep(1.0 - heather.a, 1.1 - heather.a, patches));
    }
    // Barren ground keeps its own colour on slopes loose cinder and scree hold, up to about 37°; walls
    // past about 47° are rock, in the region's cliff colour where it names one. Hard rock stands bare
    // from about 37°; soft marl and clay hold scrub on their slopes.
    var bare_from = mix(0.62, 0.766, smoothstep(0.2, 0.6, hardness));
    bare_from = mix(bare_from, min(bare_from, terrain.y), green);
    // Rock breaks into crags about 26 m across, in the plane of the face.
    // A crag that faces up holds turf where the satellite sees green.
    let rock = skirt > 0.0 || (broad > 0.0 && slope < mix(bare_from + 0.12, 0.95, banded));
    let on_face = vec2f(select(world.x, world.y, abs(n.x) > abs(n.y)), altitude);
    let big = select(0.0, 1.0 - smoothstep(4.0, 12.0, surface_size), rock);
    // Gullies and buttresses tens of metres wide and a hundred tall bend the crags.
    var buttress = vec3f(0.5, 0.0, 0.0);
    if (rock) {
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
    let walled = steep * (1.0 - g.tree);
    if (walled > 0.0) {
        let wall = rule(region, slot_cliff());
        var face = mix(mix(stone, wall.rgb, wall.a), g.albedo, g.barren * smoothstep(0.68, 0.8, n.z));
        // Lit and weathered stretches of the wall, tens of metres across.
        let stain = noise(world / 70.0 + altitude / 50.0);
        // Each crag faces a little across the face and up or down it; the gullies and buttresses tilt
        // the face itself.
        let sheer = 1.0 - smoothstep(0.3, 0.55, n.z);
        let across = normalize(vec3f(-n.y, n.x, 0.0) + 1e-6);
        let up = cross(n, across);
        var tilt = (crags.xy - 0.5) * 1.1 * big;
        var gully = 0.5;
        var gully_tilt = 0.0;
        if (sheer > 0.0 && surface_size < 25.0) {
            let wide = 1.0 - smoothstep(8.0, 25.0, surface_size);
            gully = mix(0.5, buttress.x, wide);
            gully_tilt = -0.8 * buttress.y * wide * sheer;
        }
        let plain = n;
        n = normalize(n + across * gully_tilt * walled);
        // A facet's light rises and falls as far as it turns toward the sun, and in shade toward the
        // sun's side of the sky and up; a gully sees less of the sky. Close by, the crags differ less,
        // so a near wall reads as one rough surface.
        let calm = 1.0 - smoothstep(1.0, 3.0, surface_size);
        tilt *= 1.0 - 0.35 * calm;
        let turned = across * tilt.x + up * tilt.y;
        let reach = smoothstep(-0.2, 0.1, dot(plain, look.sun.xyz));
        face *= 1.0 + 0.9 * dot(turned, look.sun.xyz) * reach * walled;
        open *= mix(1.0, (1.0 + 0.6 * tilt.x * dot(across.xy, normalize(look.sun.xy + 1e-6)) + 0.3 * tilt.y) * mix(0.75, 1.25, gully), walled);
        let contrast = 1.0 - 0.5 * calm;
        let bulge = mix(1.0, mix(0.81, 1.05, smoothstep(0.0, 0.35, crags.z)), big * contrast);
        var tone = 1.0 + 0.25 * (fract((crags.x + crags.y) * 13.7) - 0.5) * big * contrast;
        // The bedding shows only as a faint tone that comes and goes.
        let bed = hash(vec2f(floor(dot(vec3f(world, altitude), bedding) / 11.0), 3.0));
        tone *= 1.0 + 0.08 * (bed - 0.5) * (1.0 - smoothstep(0.4, 0.6, stain)) * (1.0 - smoothstep(3.0, 10.0, surface_size));
        face *= tone * bulge * (0.85 + 0.3 * stain) * mix(0.75, 1.25, gully) * (1.0 + 0.04 * sheer);
        face = mix(face, albedo, top * smoothstep(0.35, 0.6, plain.z) * smoothstep(0.3, 0.6, g.lush) * (1.0 - g.bare));
        face *= 1.0 - 0.45 * (1.0 - smoothstep(1.0, 4.0, altitude));
        face = mix(face, vec3f(0.42, 0.2, 0.06), 0.28 * smoothstep(3.0, 6.0, altitude) * (1.0 - smoothstep(10.0, 16.0, altitude))
            * smoothstep(0.45, 0.7, noise(world / 3.0 + altitude / 2.0)));
        albedo = mix(albedo, face, walled);
    }
    // Past about 1 km, the eye sees woodland's crowns, not the snow on the ground under them. Snow slides
    // off walls, which the data's texels are too wide to see.
    let crowns = g.tree * smoothstep(800.0, 1000.0, length(relative.xy));
    let snow = g.snow * (1.0 - crowns) * smoothstep(0.4, 0.6, n.z);
    albedo = mix(albedo, rule(region, i32(COVER_SNOW) * 3).rgb, snow) * (1.0 - 0.3 * look.light.w * (1.0 - snow));
    // Rock shows its relief in the smooth light: the bands would light every face toward the sun alike.
    let c = light_up(albedo, n, 1.0, open, mix(look.horizon.w, 0.5, walled));
    return finish(atmosphere(c, relative, altitude));
}
