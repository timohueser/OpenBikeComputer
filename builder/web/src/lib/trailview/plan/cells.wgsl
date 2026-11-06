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
// A sunk vertex draws this far under its height: more than the ground changes over a level-17 cell, so
// each terrain triangle with a sunk corner stays under the mesh.
const PLAN_SINK_M: f32 = 1000.0;

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

// Whether a mesh draws a grid point whole: its cell has one, and the point lies on the level-18 plateau of
// the LOD, before the band where the mesh fades into the far look and the terrain morphs. A level-18 vertex
// there takes no snap of its own: the mesh draws the ways. A vertex that morphs takes the coarser level's
// snap, as the level-17 node beside it does.
fn plan_near(u: vec2i) -> bool {
    return plan_meshed(u) && lod(lod_space(vec2f(u))) >= 18.0;
}

// Whether the terrain sinks its level-18 vertex at grid point `u`: the point lies strictly inside the owned
// region, and a mesh draws whole over every cell that holds it. A terrain triangle with a sunk corner reaches
// at most a level-18 triangle past the plateau, where the mesh still draws every pixel.
fn plan_sunk(u: vec2i) -> bool {
    let w = surface.anchor + u;
    let c = w >> vec2u(14u);
    let g = vec2u((w & vec2i(16383)) >> vec2u(6u));
    let slot = plan_slot(c);
    if (slot == 0u || lod(lod_space(vec2f(u))) < 18.0) {
        return false;
    }
    if ((g.x == 0u && plan_slot(c - vec2i(1, 0)) == 0u) || (g.y == 0u && plan_slot(c - vec2i(0, 1)) == 0u) || (all(g == vec2u(0u)) && plan_slot(c - vec2i(1, 1)) == 0u)) {
        return false;
    }
    let bit = g.y * PLAN_SIDE + g.x;
    return ((plan_cells[PLAN_HEAD + (slot - 1u) * PLAN_SUNK_WORDS + bit / 32u] >> (bit % 32u)) & 1u) == 1u;
}
