// TypeScript mirror of the normative OBCA grid. Shared test vectors pin it to the
// Rust producer. Boxes use named lat/lon fields; all arithmetic is exact integer
// microdegrees within JavaScript's safe range.

/** Origin of the fixed global cell grid, µdeg, on **both** axes.
 *
 *  A power of two, so every permitted cell size divides it exactly — which is the
 *  whole reason the grid is not anchored at −90/−180. */
export const GRID_ORIGIN = -268_435_456;

/** Side of the world box, µdeg: `2^29`, i.e. ≈ ±268.435456°. Strictly larger than
 *  the geographic domain, and the grid does **not** wrap. */
export const WORLD_SIDE = 536_870_912;

/** Smallest permitted cell size as `log2(µdeg)`. */
export const MIN_CELL_LOG2 = 10;

/** Largest permitted cell size as `log2(µdeg)`. */
export const MAX_CELL_LOG2 = 28;

/** A cell id, or a coordinate, that the grid does not admit. Thrown rather than
 *  returned as a null: every caller here works from a catalog document a producer
 *  already validated, so a failure is a bug or a corrupt document, and neither
 *  should be papered over with a fallback cell. */
export class GridError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "GridError";
    }
}

/** One cell of the grid: a size (as `log2(µdeg)`) and its **latitude** index `i`
 *  and **longitude** index `j`. The canonical id is `<log2>/<i>/<j>`, latitude
 *  first. */
export interface CellId {
    readonly log2: number;
    readonly i: number;
    readonly j: number;
}

/** A box in integer microdegrees. Half-open on `max` wherever a cell square is
 *  meant; an ordinary bbox from a user's drag is closed, and
 *  {@link cellsIntersecting} is written to make both behave the same way. */
export interface UBox {
    minLat: number;
    minLon: number;
    maxLat: number;
    maxLon: number;
}

/** Cell size in µdeg. */
export function cellSize(log2: number): number {
    return 2 ** log2;
}

/** Cells per axis at size `2^log2`. */
export function axisCells(log2: number): number {
    return WORLD_SIDE / 2 ** log2;
}

/** Zero-padding width of a cell id's indices: `max(4, digits(cells per axis − 1))`.
 *  Four for every size at or above `2^16`, wider below — producers MUST widen rather
 *  than truncate, and a consumer that assumed four would fail to key a `2^10` id
 *  against its own index. */
export function idWidth(log2: number): number {
    return Math.max(4, String(axisCells(log2) - 1).length);
}

function checkLog2(log2: number): void {
    if (!Number.isInteger(log2) || log2 < MIN_CELL_LOG2 || log2 > MAX_CELL_LOG2) {
        throw new GridError(
            `cell size 2^${log2} µdeg is outside the grid's ${MIN_CELL_LOG2}..=${MAX_CELL_LOG2}`,
        );
    }
}

/** A cell by size + indices, validated against the world box. */
export function cellId(log2: number, i: number, j: number): CellId {
    checkLog2(log2);
    const n = axisCells(log2);
    const bad = (v: number) => !Number.isInteger(v) || v < 0 || v >= n;
    if (bad(i) || bad(j)) {
        throw new GridError(
            `cell 2^${log2}/${i}/${j} is outside the world box (indices must be 0..${n - 1})`,
        );
    }
    return { log2, i, j };
}

/** Parse a canonical id `<log2>/<i>/<j>`. Lenient about zero padding on the way in,
 *  strict on the way out ({@link formatCellId} pads canonically) — the same asymmetry
 *  the Rust side has, so an id written by either round-trips through the other. */
export function parseCellId(s: string): CellId {
    const parts = s.split("/");
    if (parts.length !== 3 || parts.some((p) => !/^\d+$/.test(p))) {
        throw new GridError(`cell id ${JSON.stringify(s)} is not <log2>/<i>/<j>`);
    }
    return cellId(Number(parts[0]), Number(parts[1]), Number(parts[2]));
}

/** The canonical, zero-padded id. Also this app's map key for a cell: canonical
 *  means two spellings of one cell cannot end up as two entries. */
export function formatCellId(cell: CellId): string {
    const w = idWidth(cell.log2);
    return `${cell.log2}/${String(cell.i).padStart(w, "0")}/${String(cell.j).padStart(w, "0")}`;
}

/** The cell's square, half-open on both axes.
 *
 *  This is the whole of a cell's coverage: a catalog entry carries no bbox precisely
 *  because the id determines the square to the microdegree, and the bakery verifies
 *  the artifact's own OBCM header against it. */
export function cellSquare(cell: CellId): UBox {
    const s = cellSize(cell.log2);
    const minLat = GRID_ORIGIN + cell.i * s;
    const minLon = GRID_ORIGIN + cell.j * s;
    return { minLat, minLon, maxLat: minLat + s, maxLon: minLon + s };
}

/** Floor division — `Math.trunc` would round toward zero and drift a whole cell
 *  wherever the dividend is negative.
 *
 *  No catalog coordinate is: the origin is −2^28, strictly outside the geographic
 *  domain on both axes, so `Math.trunc` would agree everywhere a real map is. It
 *  stays because the alternative is arithmetic that is only correct because of a
 *  fact stated three definitions away, and it is pinned by an explicitly
 *  *below*-origin vector in `grid.test.ts`. */
function floorDiv(a: number, b: number): number {
    return Math.floor(a / b);
}

/** The cell of size `2^log2` whose half-open square contains `(lat, lon)`.
 *
 *  Half-open is the point: a coordinate exactly on a cell's `max` edge belongs to
 *  the **next** cell, so a point is owned by exactly one cell of a size. */
export function cellContaining(log2: number, lat: number, lon: number): CellId {
    return {
        log2,
        i: floorDiv(lat - GRID_ORIGIN, cellSize(log2)),
        j: floorDiv(lon - GRID_ORIGIN, cellSize(log2)),
    };
}

/** Whether `(lat, lon)` lies in this cell's half-open square. */
export function cellContains(cell: CellId, lat: number, lon: number): boolean {
    const { minLat, minLon, maxLat, maxLon } = cellSquare(cell);
    return lat >= minLat && lat < maxLat && lon >= minLon && lon < maxLon;
}

/**
 * How many cells one call will enumerate before it refuses.
 *
 * Not a format constant — a client one, and a chosen value. The covering of a box is
 * a product of two spans, so it grows quadratically as a map view zooms out: the
 * whole world at `2^18` is 4.2 M cells. The ceiling belongs here rather than in each
 * caller, because every caller would otherwise need the same arithmetic to guess a
 * safe box. 2^16 is far above any real selection — the whole of DACH at `2^18` is
 * ≈ 1 500 cells, all of Europe ≈ 20 000 — and far below the point where building the
 * array is what fails.
 */
export const MAX_ENUMERATED_CELLS = 65_536;

/**
 * Every cell of size `2^log2` whose square intersects `box`, in ascending `(i, j)`
 * order.
 *
 * Intersection is decided on the **half-open** squares, so the `max` edges of `box`
 * are inclusive of the cell that owns them. The grid does not wrap: a box reaching
 * past the world box is clamped to it, and a box lying entirely outside it covers
 * nothing at all rather than a clamped strip of edge cells.
 *
 * This is the coverage rule in one function, and the *generous coarse* behaviour
 * falls straight out of it: run it per band and a corridor is covered precisely at
 * `2^18` and generously at `2^20`. There is no second rule and no special case.
 *
 * Throws a {@link GridError} rather than enumerating more than `maxCells`. The count
 * is worked out from the index spans before anything is allocated, so a world-sized
 * box costs four divisions and a message, not a heap.
 */
export function cellsIntersecting(log2: number, box: UBox, maxCells = MAX_ENUMERATED_CELLS): CellId[] {
    checkLog2(log2);
    if (box.minLon > box.maxLon || box.minLat > box.maxLat) return [];
    const s = cellSize(log2);
    const n = axisCells(log2);
    const worldMax = GRID_ORIGIN + WORLD_SIDE;
    // Wholly outside the world box on either axis: nothing, not the nearest
    // edge cells. Clamping here would hand back a strip of cells covering
    // ground the box never touched.
    if (box.maxLat < GRID_ORIGIN || box.minLat >= worldMax) return [];
    if (box.maxLon < GRID_ORIGIN || box.minLon >= worldMax) return [];
    const index = (v: number) => Math.min(Math.max(floorDiv(v - GRID_ORIGIN, s), 0), n - 1);
    const i0 = index(box.minLat);
    const i1 = index(box.maxLat);
    const j0 = index(box.minLon);
    const j1 = index(box.maxLon);
    const count = (i1 - i0 + 1) * (j1 - j0 + 1);
    if (count > maxCells) {
        throw new GridError(
            `this box covers ${count} cells of size 2^${log2} µdeg, more than the ${maxCells} ` +
                "this client will enumerate at once — select a smaller area",
        );
    }
    const out: CellId[] = [];
    for (let i = i0; i <= i1; i++) {
        for (let j = j0; j <= j1; j++) out.push({ log2, i, j });
    }
    return out;
}

/** Whether `v` lies exactly on a grid line of size `2^log2`. A pure function of the
 *  coordinate, which is why two neighbours cannot disagree about it. */
export function onGridLine(v: number, log2: number): boolean {
    const s = cellSize(log2);
    return (v - GRID_ORIGIN) % s === 0;
}

/**
 * The bounding box of a set of cells' squares.
 *
 * Not the coverage outline — that is the union of the squares, drawn as its true
 * stair-edged shape (`outline.ts`). This is only what a map view has to fit, and it
 * is `null` for an empty set rather than a degenerate box at the origin.
 *
 * **Not a re-resolvable box.** Feeding it back to {@link cellsIntersecting} returns
 * the whole rectangle of cells it spans, which for anything but a solid block is a
 * strict superset of the set it came from. A cell set is the answer; this is a
 * viewport.
 */
export function coverageBbox(cells: Iterable<CellId>): UBox | null {
    let box: UBox | null = null;
    for (const cell of cells) {
        const sq = cellSquare(cell);
        if (!box) {
            box = { ...sq };
            continue;
        }
        box.minLat = Math.min(box.minLat, sq.minLat);
        box.minLon = Math.min(box.minLon, sq.minLon);
        box.maxLat = Math.max(box.maxLat, sq.maxLat);
        box.maxLon = Math.max(box.maxLon, sq.maxLon);
    }
    return box;
}
