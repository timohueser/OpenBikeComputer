// Where a feature tile is built up: its buildings' outlines in a grid of world cells, made once per tile.
// The town rules read it: the sidewalks, the paved footways, the walks to the houses and the street lamps.
// The sidewalks' slabs use the grid for the roads near them.
import type { Shapes } from './bins';

/**
 * A place is in a town where a building comes within BUILT_M metres of it. A sidewalk changes the heights
 * up to 16 m from a piece of road (its edge, its sidewalk and its cut and fill), so the buildings within
 * BUILT_M of the piece's middle, 4 m from its ends, lie in the 40 m buffer of every feature tile that
 * stamps those heights: each tile decides alike.
 */
export const BUILT_M = 20;
/** The sides of a line from a to b; world y runs south, so its right is where the cross product is positive. */
export const LEFT = 1, RIGHT = 2;
/** The granite of a town's kerbs: the ground's paint and the sidewalks' slabs. */
export const KERB = '#c3c0b8';
/** Cells of 2^SHIFT world units: about 50 m in the Alps. */
const SHIFT = 11;

/** Boxes of world units (west, north, east, south), the boxes that meet each cell, and a mark per box for a query. */
export interface Grid { boxes: number[][]; cells: Map<number, number[]>; marks: Uint32Array; query: number }
/** The first outer ring of each building in a grid of their boxes. */
export interface Town extends Grid { rings: number[][] }

const towns = new WeakMap<Shapes['buildings'], Town>();
const key = (i: number, j: number) => i * 2 ** 20 + j;

export function gridOf(boxes: number[][]): Grid {
    const cells = new Map<number, number[]>();
    boxes.forEach(([x0, y0, x1, y1], b) => {
        for (let i = Math.floor(x0) >> SHIFT; i <= Math.floor(x1) >> SHIFT; i++) for (let j = Math.floor(y0) >> SHIFT; j <= Math.floor(y1) >> SHIFT; j++) {
            const list = cells.get(key(i, j));
            if (list) list.push(b);
            else cells.set(key(i, j), [b]);
        }
    });
    return { boxes, cells, marks: new Uint32Array(boxes.length), query: 0 };
}

export function townOf(buildings: Shapes['buildings']): Town {
    let town = towns.get(buildings);
    if (town) return town;
    const rings = buildings.map(({ rings }) => rings[0]);
    const boxes = rings.map(ring => {
        const xs = ring.filter((_, i) => i % 2 === 0), ys = ring.filter((_, i) => i % 2 === 1);
        return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
    });
    towns.set(buildings, (town = { rings, ...gridOf(boxes) }));
    return town;
}

/** Calls `visit` once with each box of a grid that comes within `reach` world units of segment a-b's box. */
export function near(grid: Grid, ax: number, ay: number, bx: number, by: number, reach: number, visit: (box: number) => void): void {
    const x0 = Math.min(ax, bx) - reach, y0 = Math.min(ay, by) - reach, x1 = Math.max(ax, bx) + reach, y1 = Math.max(ay, by) + reach, mark = ++grid.query;
    for (let i = Math.floor(x0) >> SHIFT; i <= Math.floor(x1) >> SHIFT; i++) for (let j = Math.floor(y0) >> SHIFT; j <= Math.floor(y1) >> SHIFT; j++) {
        for (const b of grid.cells.get(key(i, j)) ?? []) {
            const box = grid.boxes[b];
            if (grid.marks[b] === mark || box[0] > x1 || box[2] < x0 || box[1] > y1 || box[3] < y0) continue;
            grid.marks[b] = mark;
            visit(b);
        }
    }
}

/** The share along segment a-b of the point on it nearest to x, y. */
export const along = (ax: number, ay: number, bx: number, by: number, x: number, y: number) =>
    Math.min(1, Math.max(0, ((x - ax) * (bx - ax) + (y - ay) * (by - ay)) / ((bx - ax) ** 2 + (by - ay) ** 2 || 1)));

/** Whether a building comes within `reach` world units of x, y. */
export function builtNear(town: Town, x: number, y: number, reach: number): boolean {
    let built = false;
    near(town, x, y, x, y, reach, b => {
        const ring = town.rings[b];
        for (let i = 0, j = ring.length - 2; i < ring.length && !built; j = i, i += 2) {
            const t = along(ring[j], ring[j + 1], ring[i], ring[i + 1], x, y);
            built = (ring[j] + t * (ring[i] - ring[j]) - x) ** 2 + (ring[j + 1] + t * (ring[i + 1] - ring[j + 1]) - y) ** 2 <= reach * reach;
        }
    });
    return built;
}
