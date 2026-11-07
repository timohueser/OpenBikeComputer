// The cells whose ground-plan mesh draws (pieces.ts), as the terrain's vertices and the pieces' read them: a
// window of PLAN_WINDOW × PLAN_WINDOW cells (zoom 16) from the cell in words 0 and 1, each with the slot + 1 of
// its sunk points (0 where no mesh draws), then from PLAN_HEAD per slot the cell's level-18 grid points that
// lie strictly inside its mesh's owned region, one bit each, row by row from the north-west, both borders
// included.
// #include ../terrain/drawn-mesh.wgsl
@group(0) @binding(3) var<storage, read> plan_cells: array<u32>;

// As WINDOW, HEAD, SIDE and SUNK_WORDS of pieces.ts.
const PLAN_WINDOW: i32 = 7;
const PLAN_HEAD: u32 = 64u;
const PLAN_SIDE: u32 = 257u;
const PLAN_SUNK_WORDS: u32 = 2065u;

// The slot + 1 of world cell `c`, 0 where no mesh draws.
fn plan_slot(c: vec2i) -> u32 {
    let k = c - vec2i(bitcast<i32>(plan_cells[0]), bitcast<i32>(plan_cells[1]));
    if (any(k < vec2i(0)) || any(k >= vec2i(PLAN_WINDOW))) {
        return 0u;
    }
    return plan_cells[2u + u32(k.y * PLAN_WINDOW + k.x)];
}

// Whether a mesh draws over the cell that holds a point `u` whole units about the anchor.
fn plan_meshed(u: vec2i) -> bool {
    return plan_slot((surface.anchor + u) >> vec2u(14u)) != 0u;
}

// The LOD distance (m) where level 18 starts to morph. Nearer, a mesh draws whole and the terrain sinks
// under it, over level-18 nodes and, at a lower quality, over level-17 nodes: level 17 morphs only
// twice as far out.
fn plan_sink_m() -> f32 {
    return surface.range * exp2(-18.0 - MORPH);
}

// Whether a grid point lies nearer than plan_sink_m in the LOD metric.
fn plan_within(u: vec2i) -> bool {
    return length(vec3f(lod_space(vec2f(u)), surface.above)) < plan_sink_m();
}

// Whether a mesh draws a grid point whole: its cell has one, and the point lies before the band where the
// mesh fades into the far look. A level-18 or level-17 vertex there takes no snap of its own: the mesh
// draws the ways. A vertex that morphs takes the coarser level's snap, as the level-17 node beside it does.
fn plan_near(u: vec2i) -> bool {
    return plan_within(u) && plan_meshed(u);
}

// The sunk bit of the level-18 grid point at `u` of the cell that holds it, false where no mesh draws.
fn plan_bit(u: vec2i) -> bool {
    let w = surface.anchor + u;
    let slot = plan_slot(w >> vec2u(14u));
    if (slot == 0u) {
        return false;
    }
    let g = vec2u((w & vec2i(16383)) >> vec2u(6u));
    let bit = g.y * PLAN_SIDE + g.x;
    return ((plan_cells[PLAN_HEAD + (slot - 1u) * PLAN_SUNK_WORDS + bit / 32u] >> (bit % 32u)) & 1u) == 1u;
}

// Whether the terrain sinks its vertex at grid point `u` (level 18, or level 17 at an even point), and so
// drops each triangle that has it: the point lies strictly inside the mesh's owned region, nearer than
// plan_sink_m, and a mesh draws over every cell that holds it. Every terrain triangle inside the region then
// has a sunk corner, or has all three corners on its border: an ear that the strip leaves to the terrain at
// the drawn level. A triangle with a sunk corner reaches at most a level-17 triangle past plan_sink_m, where
// the mesh still draws every pixel.
fn plan_sunk(u: vec2i) -> bool {
    let w = surface.anchor + u;
    let c = w >> vec2u(14u);
    let g = vec2u((w & vec2i(16383)) >> vec2u(6u));
    if (!plan_within(u)) {
        return false;
    }
    if ((g.x == 0u && plan_slot(c - vec2i(1, 0)) == 0u) || (g.y == 0u && plan_slot(c - vec2i(0, 1)) == 0u) || (all(g == vec2u(0u)) && plan_slot(c - vec2i(1, 1)) == 0u)) {
        return false;
    }
    return plan_bit(u);
}

// The debug view `owned`: whether the mesh draws a point of LOD space drawn by a terrain node of `level`, by
// the strip's rule. The point lies in R of a cell whose mesh draws (the middle of its region cell is sunk),
// nearer than plan_sink_m less two level-17 cells, where each terrain triangle that holds it has its corners
// sunk or on ∂R; and not in what the strip leaves to the terrain: at level 18 an ear, at level 17 a half,
// with all three corners on ∂R.
fn plan_owns(s: vec2f, level: u32) -> bool {
    if (length(vec3f(s, surface.above)) >= plan_sink_m() - 256.0 * surface.unit) {
        return false;
    }
    let at = units_of(s);
    let o = (((surface.anchor + vec2i(floor(at))) >> vec2u(7u)) << vec2u(7u)) - surface.anchor;
    if (!plan_bit(o + vec2i(64))) {
        return false;
    }
    let q = (at - vec2f(o)) / 64.0;
    var corners: array<vec2i, 3>;
    if (level >= 18u) {
        if (q.x + q.y < 1.0) {
            corners = array<vec2i, 3>(vec2i(0, 0), vec2i(0, 1), vec2i(1, 0));
        } else if (q.x >= 1.0 && q.y >= 1.0 && q.x + q.y > 3.0) {
            corners = array<vec2i, 3>(vec2i(2, 1), vec2i(1, 2), vec2i(2, 2));
        } else {
            return true;
        }
    } else if (q.x + q.y < 2.0) {
        corners = array<vec2i, 3>(vec2i(0, 0), vec2i(0, 2), vec2i(2, 0));
    } else {
        corners = array<vec2i, 3>(vec2i(2, 0), vec2i(0, 2), vec2i(2, 2));
    }
    for (var k = 0; k < 3; k++) {
        if (plan_bit(o + corners[k] * 64)) {
            return true;
        }
    }
    return false;
}
