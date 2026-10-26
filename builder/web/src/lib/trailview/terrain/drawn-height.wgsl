// The texels of the drawn surface: the LOD map gives the page of each level, and the pages give the
// heights, and the normals from them; drawn-mesh.wgsl builds the mesh and drawnHeight(s) from them. Include it
// with `// #include drawn-height.wgsl`.
//
// LOD space s is metres east and north of the eye along the Mercator axes, at the scale of the eye's
// latitude. World positions are whole units (2^30 across the world) about an anchor, so no float
// ever holds a world coordinate.

struct Surface {
    // The eye: whole world units, and the rest of a unit.
    anchor: vec2i,
    part: vec2f,
    // Metres per unit at the eye.
    unit: f32,
    // The eye's height above the ground, for the LOD.
    above: f32,
    // The distance where the LOD is 0.
    range: f32,
    // The finest level that draws.
    finest: u32,
    // The cubic from LOD space to render space (frame metres about the eye): 10 coefficients of x, then 10 of y.
    forward: array<vec4f, 5>,
    // Per level: xy the node at the origin of its LOD map window. zw is unused: a uniform array element takes 16 bytes.
    levels: array<vec4i, 19>,
}

@group(1) @binding(0) var<uniform> surface: Surface;
// MAP × MAP entries per level: the layer + 1 of the page that draws (0 while none does) in the low
// 16 bits, and of the raw stored page in the high 16 bits (0 for a synthesized page). A stored page
// with stamps draws a stamped copy.
@group(1) @binding(1) var<storage, read> lod_map: array<u32>;
// Per texel: a 16-bit code; the height is the page's offset + code × its scale. A page is one of
// 8 × 8 tiles of a layer.
@group(1) @binding(2) var heights: texture_2d_array<u32>;
@group(1) @binding(3) var<storage, read> scales: array<vec2f>;
// For textureGather only: it reads the four texels of a 2 × 2 quad in one access, unfiltered.
@group(1) @binding(4) var quad_sampler: sampler;

const MAP: i32 = 32;
// Texels across a layer: 8 pages of 256.
const LAYER: f32 = 2048.0;
const COARSEST: u32 = 1u;
const MORPH: f32 = 0.3;
// As MARGIN in lod.ts.
const MARGIN: f32 = 1e-3;
const UNKNOWN16: u32 = 0xffffu;
// A height that no page knows. Never NaN: shader compilers may assume that there is none.
const NONE: f32 = -1e30;

fn known(h: f32) -> bool {
    return h > -1e29;
}

// Units about the anchor of a point of LOD space.
fn units_of(s: vec2f) -> vec2f {
    return vec2f(s.x, -s.y) / surface.unit + surface.part;
}

// The point of LOD space at units about the anchor.
fn lod_space(u: vec2f) -> vec2f {
    let d = u - surface.part;
    return vec2f(d.x, -d.y) * surface.unit;
}

fn coefficient(i: u32) -> f32 {
    return surface.forward[i / 4u][i % 4u];
}

// The cubic of 10 coefficients from `at`: 1, x, y, x², xy, y², x³, x²y, xy², y³.
fn cubic(at: u32, p: vec2f) -> f32 {
    let x = p.x;
    let y = p.y;
    return coefficient(at) + x * (coefficient(at + 1u) + x * (coefficient(at + 3u) + x * coefficient(at + 6u) + y * coefficient(at + 7u)))
        + y * (coefficient(at + 2u) + y * (coefficient(at + 5u) + y * coefficient(at + 9u) + x * coefficient(at + 8u))) + coefficient(at + 4u) * x * y;
}

// Render space: frame metres east and north of the eye.
fn to_render(s: vec2f) -> vec2f {
    return vec2f(cubic(0u, s), cubic(10u, s));
}

// A position at a level: whole texels, and the part of a texel past the centre of the whole one.
struct Texel {
    whole: vec2i,
    part: vec2f,
}

fn texel_of(s: vec2f, level: u32) -> Texel {
    let shift = 22u - level;
    let low = vec2f(surface.anchor & vec2i((1 << shift) - 1));
    let t = (low + units_of(s)) / f32(1 << shift) - 0.5;
    let whole = floor(t);
    return Texel((surface.anchor >> vec2u(shift)) + vec2i(whole), t - whole);
}

// The same for whole world units, such as a texel centre.
fn texel_of_units(u: vec2i, level: u32) -> Texel {
    let shift = 22u - level;
    let v = u - vec2i(1 << (shift - 1u));
    return Texel(v >> vec2u(shift), vec2f(v & vec2i((1 << shift) - 1)) / f32(1 << shift));
}

// Where a texel of a page lies in its layer.
fn in_layer(page: u32, t: vec2i) -> vec2i {
    return (t & vec2i(255)) + 256 * vec2i(vec2u(page & 7u, (page >> 3u) & 7u));
}

// The LOD map entry of the node that holds a texel, or 0.
fn entry(level: u32, t: vec2i) -> u32 {
    let slot = (t >> vec2u(8u)) - surface.levels[level].xy;
    if (any(slot < vec2i(0)) || any(slot >= vec2i(MAP))) {
        return 0u;
    }
    return lod_map[(i32(level) * MAP + slot.y) * MAP + slot.x];
}

// The height of a texel: the drawn page's, or with `stored` the raw stored page's. NONE where no page holds it or it is unknown.
fn texel_height(level: u32, t: vec2i, stored: bool) -> f32 {
    let e = entry(level, t);
    let page = select(e & 0xffffu, e >> 16u, stored);
    if (page == 0u) {
        return NONE;
    }
    return slot_height(page - 1u, t);
}

// The height of a texel in the page at a layer, NONE where it is unknown.
fn slot_height(page: u32, t: vec2i) -> f32 {
    return decode(page, textureLoad(heights, in_layer(page, t), page >> 6u, 0).x);
}

// Whether the 2 × 2 quad at a texel lies in one page, so one map entry and one gather read it.
fn in_one_page(t: vec2i) -> bool {
    return all((t & vec2i(255)) < vec2i(255));
}

// The texel coordinates where textureGather reads the quad at `t`: x (0, 1), y (1, 1), z (1, 0), w (0, 0).
fn quad_at(page: u32, t: vec2i) -> vec2f {
    return (vec2f(in_layer(page, t)) + 1.0) / LAYER;
}

// The height of a raw texel of a page; NONE for unknown.
fn decode(page: u32, raw: u32) -> f32 {
    if (raw == UNKNOWN16) {
        return NONE;
    }
    let scale = scales[page];
    return scale.x + f32(raw) * scale.y;
}

// The bilinear height between four texels, or NONE unless all four are known.
fn bilinear(level: u32, t: Texel) -> f32 {
    return bilinear_rise(level, t).x;
}

// The bilinear height between four texels and its rise per texel along x and y; NONE in x unless all four are known.
fn bilinear_rise(level: u32, t: Texel) -> vec3f {
    let e = entry(level, t.whole) & 0xffffu;
    if (e == 0u) {
        return vec3f(NONE, 0.0, 0.0);
    }
    var a: f32;
    var b: f32;
    var c: f32;
    var d: f32;
    if (in_one_page(t.whole)) {
        let page = e - 1u;
        let q = textureGather(0, heights, quad_sampler, quad_at(page, t.whole), page >> 6u);
        a = decode(page, q.w);
        b = decode(page, q.z);
        c = decode(page, q.x);
        d = decode(page, q.y);
    } else {
        a = texel_height(level, t.whole, false);
        b = texel_height(level, t.whole + vec2i(1, 0), false);
        c = texel_height(level, t.whole + vec2i(0, 1), false);
        d = texel_height(level, t.whole + vec2i(1, 1), false);
    }
    if (!(known(a) && known(b) && known(c) && known(d))) {
        return vec3f(NONE, 0.0, 0.0);
    }
    return vec3f(mix(mix(a, b, t.part.x), mix(c, d, t.part.x), t.part.y), mix(b - a, d - c, t.part.y), mix(c - a, d - b, t.part.x));
}

// The height of the finest level up to `top` that knows a point at whole units: a missing height is never 0 m.
fn units_height(u: vec2i, top: u32) -> f32 {
    return units_ground(u, top).x;
}

// As units_height, with the slope there in y: the rise per metre.
fn units_ground(u: vec2i, top: u32) -> vec2f {
    for (var level = top; level >= COARSEST; level--) {
        let g = bilinear_rise(level, texel_of_units(u, level));
        if (known(g.x)) {
            return vec2f(g.x, length(g.yz) / (f32(1 << (22u - level)) * surface.unit));
        }
    }
    return vec2f(NONE, 0.0);
}

// The LOD at a point: a whole level on its plateau, and between two levels in a morph band.
fn lod(s: vec2f) -> f32 {
    let q = log2(surface.range / length(vec3f(s, surface.above)));
    let k = floor(q);
    return clamp(k - 1.0 + min(1.0, (q - k) / MORPH), f32(COARSEST), f32(surface.finest));
}

// The weights of a point in a triangle, all ≥ 0 inside it; none for a triangle without area.
fn weights(s: vec2f, a: vec2f, b: vec2f, c: vec2f) -> vec3f {
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if (area == 0.0) {
        return vec3f(-1e30);
    }
    let wb = ((s.x - a.x) * (c.y - a.y) - (s.y - a.y) * (c.x - a.x)) / area;
    let wc = ((b.x - a.x) * (s.y - a.y) - (b.y - a.y) * (s.x - a.x)) / area;
    return vec3f(1.0 - wb - wc, wb, wc);
}

// The heights of the 4 × 4 texels from t − 1 to t + 2 but its corners, a row per y from north: four
// gathers where they lie in one page, else 12 loads. NONE where no page holds a texel or its height is unknown.
fn heights_about(level: u32, page: u32, t: vec2i) -> array<vec4f, 4> {
    var rows: array<vec4f, 4>;
    let local = t & vec2i(255);
    if (all(local >= vec2i(1)) && all(local < vec2i(254))) {
        let layer = page >> 6u;
        let a = textureGather(0, heights, quad_sampler, quad_at(page, t - 1), layer);
        let b = textureGather(0, heights, quad_sampler, quad_at(page, t + vec2i(1, -1)), layer);
        let c = textureGather(0, heights, quad_sampler, quad_at(page, t + vec2i(-1, 1)), layer);
        let d = textureGather(0, heights, quad_sampler, quad_at(page, t + 1), layer);
        let scale = scales[page];
        return array<vec4f, 4>(decode4(scale, vec4u(a.wz, b.wz)), decode4(scale, vec4u(a.xy, b.xy)), decode4(scale, vec4u(c.wz, d.wz)), decode4(scale, vec4u(c.xy, d.xy)));
    }
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            if ((i == 0 || i == 3) && (j == 0 || j == 3)) {
                continue;
            }
            rows[j][i] = texel_height(level, t + vec2i(i - 1, j - 1), false);
        }
    }
    return rows;
}

// Four raw texels of a page with its offset and scale, as `decode`.
fn decode4(scale: vec2f, raw: vec4u) -> vec4f {
    return select(scale.x + vec4f(raw) * scale.y, vec4f(NONE), raw == vec4u(UNKNOWN16));
}

// The x east and y north of the unit normal of a texel, from the heights east − west and north − south of it.
fn slope_normal(east: f32, north: f32, metres: f32) -> vec2f {
    return normalize(vec3f(-east, -north, 2.0 * metres)).xy;
}

// Known heights h, else the guesses.
fn or_guess(h: vec2f, guess: vec2f) -> vec2f {
    return select(h, guess, h < vec2f(-1e29));
}

// The bilinear normal of the finest level up to `top` that knows the 4 heights of the quad of a point,
// as bilinear_rise: each texel's normal from its four neighbours. An unknown neighbour outside the quad
// continues the quad's slope, so the difference across that texel is one-sided.
fn level_normal(s: vec2f, top: u32) -> vec2f {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let e = entry(level, t.whole) & 0xffffu;
        if (e == 0u) {
            continue;
        }
        let h = heights_about(level, e - 1u, t.whole);
        // a b / c d: row 1 is the quad's north row.
        let q = vec4f(h[1].yz, h[2].yz);
        if (!known(min(min(q.x, q.y), min(q.z, q.w)))) {
            continue;
        }
        // The neighbours of a c west and of b d east, of a b north and of c d south.
        let west = or_guess(vec2f(h[1].x, h[2].x), 2.0 * q.xz - q.yw);
        let east = or_guess(vec2f(h[1].w, h[2].w), 2.0 * q.yw - q.xz);
        let north = or_guess(h[0].yz, 2.0 * q.xy - q.zw);
        let south = or_guess(h[3].yz, 2.0 * q.zw - q.xy);
        let metres = f32(1 << (22u - level)) * surface.unit;
        // East − west and north − south of the quad's texels.
        let dx = vec4f(q.y - west.x, east.x - q.x, q.w - west.y, east.y - q.z);
        let dy = vec4f(north - q.zw, q.xy - south);
        let a = slope_normal(dx.x, dy.x, metres);
        let b = slope_normal(dx.y, dy.y, metres);
        let c = slope_normal(dx.z, dy.z, metres);
        let d = slope_normal(dx.w, dy.w, metres);
        return mix(mix(a, b, t.part.x), mix(c, d, t.part.x), t.part.y);
    }
    return vec2f(0.0);
}

// The normal of the drawn surface in LOD space axes, with the same level and morph as its height.
fn drawn_normal(s: vec2f) -> vec3f {
    let lambda = lod(s);
    let level = u32(ceil(lambda));
    var n = level_normal(s, level);
    let morph = f32(level) - lambda;
    if (morph > 0.0) {
        n = mix(n, level_normal(s, level - 1u), morph);
    }
    return normalize(vec3f(n, sqrt(max(1.0 - dot(n, n), 1e-4))));
}
