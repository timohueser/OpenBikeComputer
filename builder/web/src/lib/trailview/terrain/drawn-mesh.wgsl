// The mesh as it draws: the vertices of the grid with the morph and, near roads, the snap, and the
// drawn height that every shader that stands on the terrain evaluates. Include it with
// `// #include drawn-mesh.wgsl`; it reads the feature mask.
// #include drawn-height.wgsl
// #include features.wgsl

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
// units, as the stamp pass found it for the vertex's mask: less toward the reach of the mask pool, as
// the paint.
fn snap(u: vec2i, level: u32) -> vec2i {
    if (level < MASK_LEVEL) {
        return vec2i(0);
    }
    let t = (surface.anchor + u) >> vec2u(22u - level);
    let slot = mask_of(level, t);
    if (slot == 0u) {
        return vec2i(0);
    }
    let v = (t & vec2i(255)) >> vec2u(2u);
    let word = textureLoad(snaps, v, slot - 1u, 0).x;
    let moved = vec2f(f32((bitcast<i32>(word) << 16u) >> 16u), f32(bitcast<i32>(word) >> 16u));
    let fade = 1.0 - smoothstep(0.75 * masks.reach, masks.reach, length(lod_space(vec2f(u))));
    return vec2i(round(moved * fade));
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
    // The 2 × 2 cells of a coarser cell, inside which the odd vertices slide. Where the mesh snaps, a
    // vertex moves up to 3/4 of a cell (SNAP_TEXELS of stamp.wgsl), and in the morph band up to 3/4 of
    // a coarser cell: less than one, so the coarser cell of the point and its eight neighbours hold
    // every triangle that can hold it.
    let snapped = level >= MASK_LEVEL;
    let n = select(3, 7, snapped);
    let corner = (vec2i(floor(at)) - select(vec2i(0), vec2i(1), snapped)) * block - low;
    var v: array<Vertex, 49>;
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
