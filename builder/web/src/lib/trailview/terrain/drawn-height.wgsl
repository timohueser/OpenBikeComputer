// The drawn surface: drawnHeight(s), for every shader that stands on the terrain. The LOD map gives
// the page of each level; the split rule, the LOD rule and the grid give the node, the morph and the
// triangle as the mesh draws them. Include it with `// #include drawn-height.wgsl`.
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
    // CDLOD: the distance where the LOD is 0. Rings: the ring half-width in nodes.
    range: f32,
    // The finest level that draws.
    finest: u32,
    // The cubic from LOD space to render space (frame metres about the eye): 10 coefficients of x, then 10 of y.
    forward: array<vec4f, 5>,
    // Per level: xy the node at the origin of its LOD map window, zw the ring centre in units about the anchor.
    levels: array<vec4i, 19>,
}

@group(1) @binding(0) var<uniform> surface: Surface;
// MAP × MAP entries per level: the layer + 1 of the page that draws (0 while none does) in the low
// 16 bits, and of the raw stored page in the high 16 bits (0 for a synthesized page). A stored page
// with stamps draws a stamped copy.
@group(1) @binding(1) var<storage, read> lod_map: array<u32>;
// Per texel: the bits of a float32 height, or an r16 code with an offset and a scale per page. A
// page is one of 8 × 8 tiles of a layer.
@group(1) @binding(2) var heights: texture_2d_array<u32>;
@group(1) @binding(3) var<storage, read> scales: array<vec2f>;
// Per texel: x east and y north of the unit normal.
@group(1) @binding(4) var normals: texture_2d_array<f32>;
// For textureGather only: it reads the four texels of a 2 × 2 quad in one access, unfiltered.
@group(1) @binding(5) var quad_sampler: sampler;

override RINGS: bool = false;
override R16: bool = false;

const MAP: i32 = 32;
// Texels across a layer: 8 pages of 256.
const LAYER: f32 = 2048.0;
const COARSEST: u32 = 1u;
const MORPH: f32 = 0.3;
// As MARGIN in lod.ts.
const MARGIN: f32 = 1e-3;
const UNKNOWN32: u32 = 0xffffffffu;
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
    if (R16) {
        if (raw == UNKNOWN16) {
            return NONE;
        }
        let scale = scales[page];
        return scale.x + f32(raw) * scale.y;
    }
    if (raw == UNKNOWN32) {
        return NONE;
    }
    return bitcast<f32>(raw);
}

// The bilinear height between four texels, or NONE unless all four are known.
fn bilinear(level: u32, t: Texel) -> f32 {
    let e = entry(level, t.whole) & 0xffffu;
    if (e == 0u) {
        return NONE;
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
        return NONE;
    }
    return mix(mix(a, b, t.part.x), mix(c, d, t.part.x), t.part.y);
}

// The height of the finest level up to `top` that knows a point at whole units: a missing height is never 0 m.
fn units_height(u: vec2i, top: u32) -> f32 {
    for (var level = top; level >= COARSEST; level--) {
        let h = bilinear(level, texel_of_units(u, level));
        if (known(h)) {
            return h;
        }
    }
    return NONE;
}

// The LOD at a point: a whole level on its plateau, and between two levels in a morph band.
fn lod(s: vec2f) -> f32 {
    if (RINGS) {
        return ring_lod(s);
    }
    let q = log2(surface.range / length(vec3f(s, surface.above)));
    let k = floor(q);
    return clamp(k - 1.0 + min(1.0, (q - k) / MORPH), f32(COARSEST), f32(surface.finest));
}

// The finest ring that holds the point, and its blend by the distance from the eye in nodes.
fn ring_lod(s: vec2f) -> f32 {
    let u = units_of(s);
    let e = max(abs(s.x), abs(s.y)) / surface.unit;
    for (var level = surface.finest; level > COARSEST; level--) {
        let size = f32(1 << (30u - level));
        let c = vec2f(surface.levels[level].zw);
        if (max(abs(u.x - c.x), abs(u.y - c.y)) <= surface.range * size) {
            return f32(level) - clamp((e / size - (surface.range * 0.5 + 0.55)) / (surface.range * 0.5 - 1.6), 0.0, 1.0);
        }
    }
    return f32(COARSEST);
}

// Whether the selection splits the node of a level that holds a point at `u` units about the anchor,
// as `Rule.split` in lod.ts.
fn splits(level: u32, u: vec2f) -> bool {
    if (level >= surface.finest) {
        return false;
    }
    let shift = 30u - level;
    let low = surface.anchor & vec2i((1 << shift) - 1);
    let corner = vec2f(vec2i(floor((vec2f(low) + u) / f32(1 << shift))) * (1 << shift) - low);
    let size = f32(1 << shift);
    if (RINGS) {
        let c = vec2f(surface.levels[level + 1u].zw);
        let reach = surface.range * f32(1 << (shift - 1u));
        return all(corner < c + reach) && all(corner + size > c - reach);
    }
    let near = max(max(corner - surface.part, vec2f(0.0)), surface.part - corner - size) * surface.unit;
    return log2(surface.range / length(vec3f(near, surface.above))) > f32(level) + 1.0 - MARGIN;
}

// The level of the node that draws a point of LOD space.
fn drawn_level(s: vec2f) -> u32 {
    let u = units_of(s);
    var level = COARSEST;
    while (splits(level, u)) {
        level++;
    }
    return level;
}

struct Vertex {
    s: vec2f,
    h: f32,
}

// The mesh vertex of a level at a grid point, `u` units about the anchor: an odd vertex slides onto the
// even one before it by the morph, and its height goes from its own level's to the coarser level's at
// the even vertex, so a fully morphed edge is the coarser node's edge.
fn grid_vertex(u: vec2i, level: u32) -> Vertex {
    let even = u - ((surface.anchor + u) & vec2i(1 << (24u - level)));
    let at = lod_space(vec2f(u));
    let base = lod_space(vec2f(even));
    let morph = clamp(f32(level) - lod(at), 0.0, 1.0);
    if (morph <= 0.0) {
        return Vertex(at, units_height(surface.anchor + u, level));
    }
    let coarse = units_height(surface.anchor + even, level - 1u);
    if (morph >= 1.0) {
        return Vertex(base, coarse);
    }
    let fine = units_height(surface.anchor + u, level);
    let h = select(select(fine, coarse, known(coarse)), mix(fine, coarse, morph), known(fine) && known(coarse));
    return Vertex(mix(at, base, morph), h);
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

// The drawn height at a point of LOD space: the mesh triangle that holds it, with the vertices, the
// morph and the split of the mesh. NONE where a vertex of the triangle knows no height.
fn drawn_height(s: vec2f) -> f32 {
    let level = drawn_level(s);
    // The 2 × 2 cells of a coarser cell: the odd vertices slide inside them.
    let shift = 25u - level;
    let cell = 1 << (24u - level);
    let low = surface.anchor & vec2i((1 << shift) - 1);
    let corner = vec2i(floor((vec2f(low) + units_of(s)) / f32(1 << shift))) * (1 << shift) - low;
    var v: array<Vertex, 9>;
    for (var i = 0; i < 9; i++) {
        v[i] = grid_vertex(corner + vec2i(i % 3, i / 3) * cell, level);
    }
    var best = -1e30;
    var h = NONE;
    for (var i = 0; i < 4; i++) {
        let a = i % 2 + 3 * (i / 2);
        // The two triangles of a cell, as the index buffer splits it.
        for (var t = 0; t < 2; t++) {
            let p = select(vec3i(a, a + 3, a + 1), vec3i(a + 1, a + 3, a + 4), t == 1);
            let w = weights(s, v[p.x].s, v[p.y].s, v[p.z].s);
            let least = min(w.x, min(w.y, w.z));
            if (least > best) {
                best = least;
                let known_all = known(v[p.x].h) && known(v[p.y].h) && known(v[p.z].h);
                h = select(NONE, dot(w, vec3f(v[p.x].h, v[p.y].h, v[p.z].h)), known_all);
            }
        }
    }
    return h;
}

// The normal of a texel, with z 1; z −1 where no page holds the texel or its height is unknown.
fn texel_normal(level: u32, t: vec2i) -> vec3f {
    let e = entry(level, t) & 0xffffu;
    if (e == 0u) {
        return vec3f(0.0, 0.0, -1.0);
    }
    let page = e - 1u;
    return marked(textureLoad(normals, in_layer(page, t), page >> 6u, 0).xy);
}

// A normal with z 1, or z −1 for the code of an unknown height.
fn marked(n: vec2f) -> vec3f {
    return vec3f(n, select(1.0, -1.0, n.x < -0.99 && n.y < -0.99));
}

fn level_normal(s: vec2f, top: u32) -> vec2f {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let e = entry(level, t.whole) & 0xffffu;
        if (e == 0u) {
            continue;
        }
        var a: vec3f;
        var b: vec3f;
        var c: vec3f;
        var d: vec3f;
        if (in_one_page(t.whole)) {
            let page = e - 1u;
            let x = textureGather(0, normals, quad_sampler, quad_at(page, t.whole), page >> 6u);
            let y = textureGather(1, normals, quad_sampler, quad_at(page, t.whole), page >> 6u);
            a = marked(vec2f(x.w, y.w));
            b = marked(vec2f(x.z, y.z));
            c = marked(vec2f(x.x, y.x));
            d = marked(vec2f(x.y, y.y));
        } else {
            a = texel_normal(level, t.whole);
            b = texel_normal(level, t.whole + vec2i(1, 0));
            c = texel_normal(level, t.whole + vec2i(0, 1));
            d = texel_normal(level, t.whole + vec2i(1, 1));
        }
        if (min(min(a.z, b.z), min(c.z, d.z)) > 0.0) {
            return mix(mix(a.xy, b.xy, t.part.x), mix(c.xy, d.xy, t.part.x), t.part.y);
        }
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
