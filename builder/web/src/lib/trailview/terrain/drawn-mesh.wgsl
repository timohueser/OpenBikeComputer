// The mesh as it draws: the vertices of the grid with the morph and, near roads, the snap, and the
// drawn height that every shader that stands on the terrain evaluates. Include it with
// `// #include drawn-mesh.wgsl`; it reads the feature mask.
// #include drawn-height.wgsl
// #include features.wgsl

// Whether the vertices of the levels with masks move onto the road edges, so that the mesh folds
// where the paint ends: the lab `edges`.
override SNAP: bool = false;
// A vertex moves onto a road edge within this many metres: 3/4 of a cell of level 18, so that every
// triangle along an edge has a vertex on it there, and less than half a cell of level 17.
const SNAP_M: f32 = 1.2;

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

// The move of the vertex of a level at `u` units about the anchor onto the nearest road edge, in whole
// units: from the distance to the edge that the mask holds at the four texels whose corner the vertex
// is, a quarter texel inside the road, where the texels about it hold the road's height. None where a
// texel has no mask or no road, and less toward the reach of the mask pool, as the paint.
fn snap(u: vec2i, level: u32) -> vec2i {
    if (!SNAP || level < MASK_LEVEL) {
        return vec2i(0);
    }
    let shift = 22u - level;
    let t = (surface.anchor + u) >> vec2u(shift);
    var d: array<f32, 4>;
    for (var i = 0; i < 4; i++) {
        let at = t - vec2i(1 - i % 2, 1 - i / 2);
        let slot = mask_of(level, at);
        if (slot == 0u) {
            return vec2i(0);
        }
        let texel = shape_at(slot - 1u, at);
        if ((u32(round(texel.w * 255.0)) & 7u) == 0u) {
            return vec2i(0);
        }
        d[i] = mask_metres(texel.x);
    }
    // Metres per texel, and the gradient of the distance per metre along the axes of the units.
    let metres = surface.unit * f32(1 << shift);
    let g = vec2f(d[1] + d[3] - d[0] - d[2], d[2] + d[3] - d[0] - d[1]) * 0.5 / metres;
    let mean = 0.25 * (d[0] + d[1] + d[2] + d[3]);
    let g2 = dot(g, g);
    // Off the ridges of the distance, where it turns from one edge to another.
    if (g2 < 0.25 || g2 > 4.0 || abs(mean) > SNAP_M) {
        return vec2i(0);
    }
    let fade = 1.0 - smoothstep(0.75 * masks.reach, masks.reach, length(lod_space(vec2f(u))));
    // A distance grows by a metre per metre, so the step takes only the direction of the quantized
    // gradient: by its length, it overshoots or falls short, and the triangles fold on the edge.
    let shift_m = -(mean + 0.25 * metres) * g / sqrt(g2);
    let snapped = vec2i(round(shift_m * min(1.0, SNAP_M / length(shift_m)) * fade / surface.unit));
    // A vertex on an edge of its node moves only along it: every border between two levels lies on
    // an edge of the coarser node, so each node draws exactly its own area, as drawn_level finds it.
    return select(snapped, vec2i(0), ((surface.anchor + u) & vec2i((1 << (30u - level)) - 1)) == vec2i(0));
}

// The mesh vertex of a level at a grid point, `u` units about the anchor: an odd vertex slides onto the
// even one before it by the morph, and its height goes from its own level's to the coarser level's at
// the even vertex, so a fully morphed edge is the coarser node's edge. Each level snaps its own
// vertices; the morph keeps to the grid point, so neighbouring nodes meet as without the snap.
fn grid_vertex(u: vec2i, level: u32) -> Vertex {
    let even = u - ((surface.anchor + u) & vec2i(1 << (24u - level)));
    let morph = clamp(f32(level) - lod(lod_space(vec2f(u))), 0.0, 1.0);
    let moved = u + snap(u, level);
    let at = lod_space(vec2f(moved));
    if (morph <= 0.0) {
        return Vertex(at, units_height(surface.anchor + moved, level));
    }
    let settled = even + snap(even, level - 1u);
    let base = lod_space(vec2f(settled));
    let coarse = units_height(surface.anchor + settled, level - 1u);
    if (morph >= 1.0) {
        return Vertex(base, coarse);
    }
    let fine = units_height(surface.anchor + moved, level);
    let h = select(select(fine, coarse, known(coarse)), mix(fine, coarse, morph), known(fine) && known(coarse));
    return Vertex(mix(at, base, morph), h);
}

// The drawn height at a point of LOD space: the mesh triangle that holds it, with the vertices, the
// morph and the split of the mesh. NONE where a vertex of the triangle knows no height.
fn drawn_height(s: vec2f) -> f32 {
    let level = drawn_level(s);
    let cell = 1 << (24u - level);
    let block = 2 * cell;
    let low = surface.anchor & vec2i(block - 1);
    let at = (vec2f(low) + units_of(s)) / f32(block);
    // The 2 × 2 cells of a coarser cell, inside which the odd vertices slide. With the snap, the 2 × 2
    // coarser cells nearest the point: a snapped even vertex takes the cell that a morphed odd vertex
    // stretches by up to SNAP_M into the next coarser cell, and SNAP_M is less than half of one.
    let snapped = SNAP && level >= MASK_LEVEL;
    let n = select(3, 5, snapped);
    let before = select(vec2i(0), select(vec2i(0), vec2i(block), fract(at) < vec2f(0.5)), snapped);
    let corner = vec2i(floor(at)) * block - low - before;
    var v: array<Vertex, 25>;
    for (var i = 0; i < n * n; i++) {
        v[i] = grid_vertex(corner + vec2i(i % n, i / n) * cell, level);
    }
    var best = -1e30;
    var h = NONE;
    for (var i = 0; i < (n - 1) * (n - 1); i++) {
        let a = i % (n - 1) + n * (i / (n - 1));
        // The two triangles of a cell, as the index buffer splits it.
        for (var t = 0; t < 2; t++) {
            let p = select(vec3i(a, a + n, a + 1), vec3i(a + 1, a + n, a + n + 1), t == 1);
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
