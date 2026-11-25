// Where a feature tile is built up: its buildings' outlines in a grid of world cells, made once per tile.
// The town rules read it: the sidewalks, the paved footways, the houses' fronts and the street lamps.
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
/** The granite of a town's kerbs: the ground's paint and the corridors. */
export const KERB = '#c3c0b8';
/** Cells of 2^SHIFT world units: about 50 m in the Alps. */
const SHIFT = 11;

/** Boxes of world units (west, north, east, south), the boxes that meet each cell of 2^shift units (SHIFT without one), and a mark per box for a query. */
export interface Grid { boxes: number[][]; cells: Map<number, number[]>; marks: Uint32Array; query: number; shift?: number }
/** The first outer ring of each building in a grid of their boxes. */
export interface Town extends Grid { rings: number[][] }

const towns = new WeakMap<Shapes['buildings'], Town>();
const key = (i: number, j: number) => i * 2 ** 20 + j;

export function gridOf(boxes: number[][], shift = SHIFT): Grid {
    const grid: Grid = { boxes: [], cells: new Map(), marks: new Uint32Array(boxes.length), query: 0, shift };
    for (const box of boxes) add(grid, box);
    return grid;
}

/** Adds a box to a grid: a query after it finds the box. */
export function add(grid: Grid, box: number[]): void {
    const [x0, y0, x1, y1] = box, b = grid.boxes.push(box) - 1, shift = grid.shift ?? SHIFT;
    if (b >= grid.marks.length) {
        const marks = new Uint32Array(2 * b + 16);
        marks.set(grid.marks);
        grid.marks = marks;
    }
    for (let i = Math.floor(x0) >> shift; i <= Math.floor(x1) >> shift; i++) for (let j = Math.floor(y0) >> shift; j <= Math.floor(y1) >> shift; j++) {
        const list = grid.cells.get(key(i, j));
        if (list) list.push(b);
        else grid.cells.set(key(i, j), [b]);
    }
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
    const x0 = Math.min(ax, bx) - reach, y0 = Math.min(ay, by) - reach, x1 = Math.max(ax, bx) + reach, y1 = Math.max(ay, by) + reach, mark = ++grid.query, shift = grid.shift ?? SHIFT;
    for (let i = Math.floor(x0) >> shift; i <= Math.floor(x1) >> shift; i++) for (let j = Math.floor(y0) >> shift; j <= Math.floor(y1) >> shift; j++) {
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

/**
 * A quarter of houses: a building of at most HOUSE_M2 counts, a hall or a whole block does not. Its cover is taken over
 * cells of 2^COVER_SHIFT world units, about 26 m in the Alps, COVER_CELLS to each side of a place's cell: about 130 m.
 */
const HOUSE_M2 = 1000, COVER_SHIFT = 10, COVER_CELLS = 2;
const covers = new WeakMap<Town, Map<number, number>>();

/**
 * The share of the ground about x, y that houses cover, each house counted whole in the cell of its box's middle.
 * `unit` is the metres per world unit there. Only the cells whose middle lies within `reach` world units of `square`
 * (west, north, east, south) count: a tile holds the buildings of its square and its buffer, so two tiles see a place
 * near their border alike.
 */
export function houseCover(town: Town, x: number, y: number, unit: number, square?: number[], reach = 0): number {
    let cover = covers.get(town);
    if (!cover) {
        covers.set(town, (cover = new Map()));
        town.rings.forEach((ring, b) => {
            let twice = 0;
            for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) twice += ring[j] * ring[i + 1] - ring[i] * ring[j + 1];
            const area = Math.abs(twice) / 2, [x0, y0, x1, y1] = town.boxes[b], cell = key(Math.floor((x0 + x1) / 2) >> COVER_SHIFT, Math.floor((y0 + y1) / 2) >> COVER_SHIFT);
            if (area * unit * unit <= HOUSE_M2) cover!.set(cell, (cover!.get(cell) ?? 0) + area / 4 ** COVER_SHIFT);
        });
    }
    const size = 2 ** COVER_SHIFT, ci = Math.floor(x) >> COVER_SHIFT, cj = Math.floor(y) >> COVER_SHIFT;
    let sum = 0, cells = 0;
    for (let i = ci - COVER_CELLS; i <= ci + COVER_CELLS; i++) for (let j = cj - COVER_CELLS; j <= cj + COVER_CELLS; j++) {
        const mx = (i + 0.5) * size, my = (j + 0.5) * size;
        if (square && (mx < square[0] - reach || mx > square[2] + reach || my < square[1] - reach || my > square[3] + reach)) continue;
        sum += cover.get(key(i, j)) ?? 0;
        cells++;
    }
    return cells ? sum / cells : 0;
}

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
