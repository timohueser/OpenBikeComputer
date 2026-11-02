// The mesh as it draws: the vertices of the grid with the morph, near roads the snap, and where a way owns the
// ground the clamp under its corridor; and the drawn height that every shader that stands on the terrain
// evaluates. Include it with `// #include drawn-mesh.wgsl`; it reads the feature mask.
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

// The move of the vertex of a level at `u` units about the anchor onto the nearest road edge, in whole units, and
// its drop under the corridor of the way that owns its ground, in metres, as the snap pass found them for the
// vertex's mask (snap-word.wgsl). The move lessens toward the reach of the mask pool, as the paint.
struct Snap {
    moved: vec2i,
    drop: f32,
}

fn snap(u: vec2i, level: u32, snapping: bool) -> Snap {
    if (!snapping || level < MASK_LEVEL) {
        return Snap(vec2i(0), 0.0);
    }
    let t = (surface.anchor + u) >> vec2u(22u - level);
    let slot = mask_of(level, t);
    if (slot == 0u) {
        return Snap(vec2i(0), 0.0);
    }
    let v = (t & vec2i(255)) >> vec2u(2u);
    let word = textureLoad(snaps, v, slot - 1u, 0).x;
    let fade = 1.0 - smoothstep(0.75 * masks.reach, masks.reach, length(lod_space(vec2f(u))));
    return Snap(vec2i(round(vec2f(snap_moved(word)) * fade)), snap_drop(word));
}

// Where the mesh vertex of a level at a grid point, `u` units about the anchor, lies: an odd vertex
// slides onto the even one before it by the morph. Each level snaps its own vertices; the morph keeps
// to the grid point, so neighbouring nodes meet as without the snap. `drops` holds the drops at `moved` and at `settled`.
struct Place {
    at: vec2f,
    moved: vec2i,
    settled: vec2i,
    morph: f32,
    drops: vec2f,
}

fn vertex_place(u: vec2i, level: u32, snapping: bool) -> Place {
    let morph = clamp(f32(level) - lod(lod_space(vec2f(u))), 0.0, 1.0);
    let fine = snap(u, level, snapping);
    let moved = u + fine.moved;
    let at = lod_space(vec2f(moved));
    if (morph <= 0.0) {
        return Place(at, moved, moved, morph, vec2f(fine.drop));
    }
    let even = u - ((surface.anchor + u) & vec2i(1 << (24u - level)));
    let coarse = snap(even, level - 1u, snapping);
    let settled = even + coarse.moved;
    return Place(mix(at, lod_space(vec2f(settled)), morph), moved, settled, morph, vec2f(fine.drop, coarse.drop));
}

// The height of a placed vertex: from its own level's to the coarser level's at the even vertex by the
// morph, so a fully morphed edge is the coarser node's edge. With `clamped`, each lies its drop under its height
// where a way owns the ground: a triangle of such vertices lies under the ground that the corridor draws over it.
// The drop fades as the paint takes the way over from the corridors.
fn place_height(p: Place, level: u32, clamped: bool) -> f32 {
    let drops = select(vec2f(0.0), p.drops * (1.0 - paint_share(length(p.at))), clamped);
    if (p.morph <= 0.0) {
        return units_height(surface.anchor + p.moved, level) - drops.x;
    }
    let coarse = units_height(surface.anchor + p.settled, level - 1u) - drops.y;
    if (p.morph >= 1.0) {
        return coarse;
    }
    let fine = units_height(surface.anchor + p.moved, level) - drops.x;
    return select(select(fine, coarse, known(coarse)), mix(fine, coarse, p.morph), known(fine) && known(coarse));
}

// The mesh vertex of a level at a grid point.
fn grid_vertex(u: vec2i, level: u32, snapping: bool, clamped: bool) -> Vertex {
    let p = vertex_place(u, level, snapping);
    return Vertex(p.at, place_height(p, level, clamped));
}

// The drawn height at a point of LOD space: where a way owns the ground, the stamped ground that its corridor
// draws; elsewhere the mesh triangle that holds the point, with the vertices, the morph and the split of the
// mesh. NONE where no page knows it.
fn drawn_height(s: vec2f) -> f32 {
    let mesh = mesh_height(s, true, false);
    if (!owned(s, length(s))) {
        return mesh;
    }
    return stamped_height(s, mesh);
}

// The height of the mesh as the terrain draws it, with the clamp: the census compares the corridors' faces with it.
fn clamped_height(s: vec2f) -> f32 {
    return mesh_height(s, true, true);
}

// The height of the mesh without the snap: within a few texels of a road edge it may lie a few cm
// off the drawn mesh, and it reads a ninth of the vertices. The water away from the roads drapes on it (water_ground).
fn plain_height(s: vec2f) -> f32 {
    return mesh_height(s, false, false);
}

fn mesh_height(s: vec2f, snapping: bool, clamped: bool) -> f32 {
    let level = drawn_level(s);
    let cell = 1 << (24u - level);
    let block = 2 * cell;
    let low = surface.anchor & vec2i(block - 1);
    let at = (vec2f(low) + units_of(s)) / f32(block);
    // The 2 × 2 cells of a coarser cell, inside which the odd vertices slide. Where the mesh snaps, a
    // vertex moves up to 15/16 of a cell (SNAP_OUT_TEXELS of stamp.wgsl), and in the morph band up to 15/16
    // of a coarser cell: less than one, so the coarser cell of the point and its eight neighbours hold
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
        h[k] = place_height(vertex_place(u, level, snapping), level, clamped);
    }
    if (!(known(h.x) && known(h.y) && known(h.z))) {
        return NONE;
    }
    return dot(w, h);
}

// A thing on the drawn surface nearer than this stands on the drawn height; a farther one on the
// bilinear heights of the two levels that the mesh blends there, which meet the mesh within a pixel.
const MESH_HEIGHT_M: f32 = 400.0;

// The ground under a point of LOD space, `u` in whole world units: where a way owns it, the stamped ground that its
// corridor draws; elsewhere where the mesh draws it.
fn stand_height(s: vec2f, u: vec2i) -> f32 {
    var ground: f32;
    if (length(s) < MESH_HEIGHT_M) {
        ground = plain_height(s);
    } else {
        ground = morphed_ground(u, lod(s)).x;
    }
    if (!owned(s, length(s))) {
        return ground;
    }
    return stamped_height(s, ground);
}

// The level whose slope a crown stands on (slope_sink of emit.wgsl): texels of 3.3 m at 47° N, about
// a crown's size. The coarse cover (COVER of lod.ts) holds its pages to the eye.
const SLOPE_LEVEL: f32 = 15.0;

// As stand_height, with the slope in y, the rise per metre: of SLOPE_LEVEL, so that it holds still as
// the eye moves; where the mesh draws coarser, of the drawn levels by their morph, so that it changes
// with the distance as the ground does.
fn stand_ground(s: vec2f, u: vec2i) -> vec2f {
    let q = lod(s);
    let slope = morphed_ground(u, min(q, SLOPE_LEVEL));
    if (q <= SLOPE_LEVEL && length(s) >= MESH_HEIGHT_M) {
        return slope;
    }
    return vec2f(stand_height(s, u), slope.y);
}

// The bilinear height and slope at a LOD `q`, as grid_vertex: a node of level `fine` goes to the
// coarser level's by its morph.
fn morphed_ground(u: vec2i, q: f32) -> vec2f {
    let fine = u32(ceil(q));
    let h = units_ground(u, fine);
    let morph = f32(fine) - q;
    if (morph <= 0.0) {
        return h;
    }
    let coarse = units_ground(u, fine - 1u);
    return select(select(h, coarse, known(coarse.x)), mix(h, coarse, morph), known(h.x) && known(coarse.x));
}
