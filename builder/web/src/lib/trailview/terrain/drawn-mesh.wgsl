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
fn snap(u: vec2i, level: u32, snapping: bool) -> vec2i {
    if (!snapping || level < MASK_LEVEL) {
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

// Where the mesh vertex of a level at a grid point, `u` units about the anchor, lies: an odd vertex
// slides onto the even one before it by the morph. Each level snaps its own vertices; the morph keeps
// to the grid point, so neighbouring nodes meet as without the snap.
struct Place {
    at: vec2f,
    moved: vec2i,
    settled: vec2i,
    morph: f32,
}

fn vertex_place(u: vec2i, level: u32, snapping: bool) -> Place {
    let morph = clamp(f32(level) - lod(lod_space(vec2f(u))), 0.0, 1.0);
    let moved = u + snap(u, level, snapping);
    let at = lod_space(vec2f(moved));
    if (morph <= 0.0) {
        return Place(at, moved, moved, morph);
    }
    let even = u - ((surface.anchor + u) & vec2i(1 << (24u - level)));
    let settled = even + snap(even, level - 1u, snapping);
    return Place(mix(at, lod_space(vec2f(settled)), morph), moved, settled, morph);
}

// The height of a placed vertex: from its own level's to the coarser level's at the even vertex by the
// morph, so a fully morphed edge is the coarser node's edge.
fn place_height(p: Place, level: u32) -> f32 {
    if (p.morph <= 0.0) {
        return units_height(surface.anchor + p.moved, level);
    }
    let coarse = units_height(surface.anchor + p.settled, level - 1u);
    if (p.morph >= 1.0) {
        return coarse;
    }
    let fine = units_height(surface.anchor + p.moved, level);
    return select(select(fine, coarse, known(coarse)), mix(fine, coarse, p.morph), known(fine) && known(coarse));
}

// The mesh vertex of a level at a grid point.
fn grid_vertex(u: vec2i, level: u32, snapping: bool) -> Vertex {
    let p = vertex_place(u, level, snapping);
    return Vertex(p.at, place_height(p, level));
}

// The drawn height at a point of LOD space: the mesh triangle that holds it, with the vertices, the
// morph and the split of the mesh. NONE where a vertex of the triangle knows no height.
fn drawn_height(s: vec2f) -> f32 {
    return mesh_height(s, true);
}

// The height of the mesh without the snap: within a few texels of a road edge it may lie a few cm
// off the drawn mesh, and it reads a ninth of the vertices. The water drapes on it.
fn plain_height(s: vec2f) -> f32 {
    return mesh_height(s, false);
}

fn mesh_height(s: vec2f, snapping: bool) -> f32 {
    return mesh_ground(s, snapping).x;
}

// As mesh_height, with the slope of the triangle in y: the rise per metre.
fn mesh_ground(s: vec2f, snapping: bool) -> vec2f {
    let level = drawn_level(s);
    let cell = 1 << (24u - level);
    let block = 2 * cell;
    let low = surface.anchor & vec2i(block - 1);
    let at = (vec2f(low) + units_of(s)) / f32(block);
    // The 2 × 2 cells of a coarser cell, inside which the odd vertices slide. Where the mesh snaps, a
    // vertex moves up to 3/4 of a cell (SNAP_TEXELS of stamp.wgsl), and in the morph band up to 3/4 of
    // a coarser cell: less than one, so the coarser cell of the point and its eight neighbours hold
    // every triangle that can hold it.
    let snapped = snapping && level >= MASK_LEVEL;
    let n = select(3, 7, snapped);
    let corner = (vec2i(floor(at)) - select(vec2i(0), vec2i(1), snapped)) * block - low;
    // The triangle that holds the point by the vertices' places, then the heights of its three only.
    var places: array<vec2f, 49>;
    for (var i = 0; i < n * n; i++) {
        places[i] = vertex_place(corner + vec2i(i % n, i / n) * cell, level, snapping).at;
    }
    var best = -1e30;
    var held = vec3i(0);
    var w = vec3f(0.0);
    for (var i = 0; i < (n - 1) * (n - 1); i++) {
        let a = i % (n - 1) + n * (i / (n - 1));
        // The two triangles of a cell, as the index buffer splits it.
        for (var t = 0; t < 2; t++) {
            let p = select(vec3i(a, a + n, a + 1), vec3i(a + 1, a + n, a + n + 1), t == 1);
            let weight = weights(s, places[p.x], places[p.y], places[p.z]);
            let least = min(weight.x, min(weight.y, weight.z));
            if (least > best) {
                best = least;
                held = p;
                w = weight;
            }
        }
    }
    var h = vec3f(0.0);
    for (var k = 0; k < 3; k++) {
        let u = corner + vec2i(held[k] % n, held[k] / n) * cell;
        h[k] = place_height(vertex_place(u, level, snapping), level);
    }
    if (!(known(h.x) && known(h.y) && known(h.z))) {
        return vec2f(NONE, 0.0);
    }
    let e1 = places[held.y] - places[held.x];
    let e2 = places[held.z] - places[held.x];
    let rise = vec2f((h.y - h.x) * e2.y - (h.z - h.x) * e1.y, e1.x * (h.z - h.x) - e2.x * (h.y - h.x)) / (e1.x * e2.y - e1.y * e2.x);
    return vec2f(dot(w, h), length(rise));
}

// A thing on the drawn surface nearer than this stands on the drawn height; a farther one on the
// bilinear heights of the two levels that the mesh blends there, which meet the mesh within a pixel.
const MESH_HEIGHT_M: f32 = 400.0;

// The ground under a point of LOD space, `u` in whole world units: where the mesh draws it.
fn stand_height(s: vec2f, u: vec2i) -> f32 {
    return stand_ground(s, u).x;
}

// As stand_height, with the slope there in y: the rise per metre.
fn stand_ground(s: vec2f, u: vec2i) -> vec2f {
    if (length(s) < MESH_HEIGHT_M) {
        return mesh_ground(s, false);
    }
    // As grid_vertex: a node of level `fine` goes to the coarser level's height by its morph.
    let q = lod(s);
    let fine = u32(ceil(q));
    let h = units_ground(u, fine);
    let morph = f32(fine) - q;
    if (morph <= 0.0) {
        return h;
    }
    let coarse = units_ground(u, fine - 1u);
    return select(select(h, coarse, known(coarse.x)), mix(h, coarse, morph), known(h.x) && known(coarse.x));
}
