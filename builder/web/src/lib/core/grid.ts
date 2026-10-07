import { core } from "./bridge";

export interface CellId { readonly log2: number; readonly i: number; readonly j: number }
export interface UBox { minLat: number; minLon: number; maxLat: number; maxLon: number }

export class GridError extends Error {
    constructor(message: string) { super(message); this.name = "GridError"; }
}

export const [GRID_ORIGIN, WORLD_SIDE, MIN_CELL_LOG2, MAX_CELL_LOG2, MAX_ENUMERATED_CELLS] = core().obc_builder_constants();

function call<T>(fn: () => T): T {
    try { return fn(); } catch (cause) {
        if (cause instanceof Error && cause.name === "GridError") throw new GridError(cause.message);
        throw cause;
    }
}

function cell(flat: ArrayLike<number>, at = 0): CellId { return { log2: flat[at], i: flat[at + 1], j: flat[at + 2] }; }
function box(flat: ArrayLike<number>): UBox { return { minLat: flat[0], minLon: flat[1], maxLat: flat[2], maxLon: flat[3] }; }

export const cellSize = (log2: number): number => core().obc_grid_cell_size(log2);
export const axisCells = (log2: number): number => core().obc_grid_axis_cells(log2);
export const idWidth = (log2: number): number => core().obc_grid_id_width(log2);
export const cellId = (log2: number, i: number, j: number): CellId => call(() => cell(core().obc_grid_cell_id(log2, i, j)));
export const parseCellId = (id: string): CellId => call(() => cell(core().obc_grid_parse_id(id)));
export const formatCellId = ({ log2, i, j }: CellId): string => core().obc_grid_format_id(log2, i, j);
export const cellSquare = ({ log2, i, j }: CellId): UBox => box(core().obc_grid_square(log2, i, j));
export const cellContaining = (log2: number, lat: number, lon: number): CellId => cell(core().obc_grid_containing(log2, lat, lon));
export const cellContains = ({ log2, i, j }: CellId, lat: number, lon: number): boolean => core().obc_grid_contains(log2, i, j, lat, lon);
export const onGridLine = (value: number, log2: number): boolean => core().obc_grid_on_line(value, log2);

export function cellsIntersecting(log2: number, bounds: UBox, maxCells = MAX_ENUMERATED_CELLS): CellId[] {
    const flat = call(() => core().obc_grid_intersecting(log2, bounds.minLat, bounds.minLon, bounds.maxLat, bounds.maxLon, maxCells));
    const cells: CellId[] = [];
    for (let at = 0; at < flat.length; at += 3) cells.push(cell(flat, at));
    return cells;
}

export function coverageBbox(cells: Iterable<CellId>): UBox | null {
    const flat = Float64Array.from(Array.from(cells, ({ log2, i, j }) => [log2, i, j]).flat());
    const bounds = core().obc_grid_coverage(flat);
    return bounds.length ? box(bounds) : null;
}
