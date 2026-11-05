// The floors of the ground plan's mesh (mesh.ts) in the terrain's vertex stage: a window of cells about
// the eye, each with a height per level-18 grid point under which the terrain stays, so that no terrain
// rises through the mesh. Words: the window's north-west cell (x, y), then per cell of the window 1 where
// it holds floors, then from FLOOR_HEAD the floors of each cell, f32, row by row from the north-west.
@group(0) @binding(3) var<storage, read> floors: array<u32>;

// As WINDOW, FLOOR_SIDE and FLOOR_HEAD of mesh.ts.
const FLOOR_WINDOW: i32 = 7;
const FLOOR_SIDE: i32 = 257;
const FLOOR_HEAD: u32 = 64u;
const NO_FLOOR: f32 = 1e30;
// World units of a cell and of a level-18 grid cell.
const CELL_UNITS: i32 = 16384;
const FLOOR_STEP: i32 = 64;

fn floor_in(g: vec2i, cell: vec2i) -> f32 {
    let k = cell - vec2i(bitcast<i32>(floors[0]), bitcast<i32>(floors[1]));
    if (any(k < vec2i(0)) || any(k >= vec2i(FLOOR_WINDOW))) {
        return NO_FLOOR;
    }
    let slot = u32(k.y * FLOOR_WINDOW + k.x);
    if (floors[2u + slot] == 0u) {
        return NO_FLOOR;
    }
    let i = (g - cell * CELL_UNITS) / FLOOR_STEP;
    return bitcast<f32>(floors[FLOOR_HEAD + slot * u32(FLOOR_SIDE * FLOOR_SIDE) + u32(i.y * FLOOR_SIDE + i.x)]);
}

// Two cells' floors at a point of their border: each holds its own side, so the lower, and none where
// one has none: the mesh ends there.
fn floor_both(a: f32, b: f32) -> f32 {
    return select(min(a, b), NO_FLOOR, max(a, b) >= NO_FLOOR);
}

// The floor at level-18 grid point `g` in whole world units, NO_FLOOR where none holds.
fn floor_at(g: vec2i) -> f32 {
    let cell = g / CELL_UNITS;
    let border = (g % CELL_UNITS) == vec2i(0);
    var f = floor_in(g, cell);
    if (border.x) {
        f = floor_both(f, floor_in(g, cell - vec2i(1, 0)));
    }
    if (border.y) {
        f = floor_both(f, floor_in(g, cell - vec2i(0, 1)));
    }
    if (border.x && border.y) {
        f = floor_both(f, floor_in(g, cell - vec2i(1, 1)));
    }
    return f;
}
