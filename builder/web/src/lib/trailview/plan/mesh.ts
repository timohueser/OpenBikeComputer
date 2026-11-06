// A cell's near mesh as the plan worker hands it over (`CellMesh::to_bytes` of `mesh/mod.rs` in
// obc-ground-plan): the one reader of those bytes, in the form that pieces.ts uploads.

const MAGIC = 0x4f424d02;
const HEAD = 9;
/** Region cells (2 × 2 grid cells) along a side of a cell, as `params::CELLS`, and the words of R on the cell and one ring. */
const CELLS = 128;
const REGION_WORDS = Math.ceil(((CELLS + 2) * (CELLS + 2)) / 32);
/** Grid points along a side of a cell, both borders included, and the words of one bit each. */
export const SIDE = 2 * CELLS + 1;
export const SUNK_WORDS = Math.floor((SIDE * SIDE) / 32) + 1;

export interface CellMesh {
    /** The cell, zoom 16. */
    x: number;
    y: number;
    /** Two words per corner: x and y in frame units from the cell's origin, the label and the terrain-vertex flag. */
    corners: Uint32Array<ArrayBuffer>;
    /** Three corner indices per triangle, the strip's among them. */
    triangles: Uint32Array<ArrayBuffer>;
    /** Four corner indices per step slot: a_p, a_q, b_q, b_p. */
    steps: Uint32Array<ArrayBuffer>;
    /** One bit per grid point (j × SIDE + i) whose region cells all lie in R: the terrain sinks there. */
    sunk: Uint32Array<ArrayBuffer>;
}

export function cellMesh(bytes: ArrayBuffer): CellMesh {
    const words = new Uint32Array(bytes);
    const [magic, x, y, corners, triangles, , triangleRuns, steps, stepRuns] = words.subarray(0, HEAD);
    if (magic !== MAGIC) throw new Error(`not a ground plan mesh: ${magic.toString(16)}`);
    let at = HEAD;
    const take = (n: number) => words.subarray(at, (at += n));
    const corner = words.slice(at, (at += 2 * corners));
    /** Items of `size` corners in runs of (count, base), their 16-bit indices less the base. */
    const items = (count: number, runs: number, size: number) => {
        const table = take(2 * runs), short = new Uint16Array(bytes, 4 * at, size * count), out = new Uint32Array(size * count);
        at += Math.ceil((size * count) / 2);
        let k = 0;
        for (let r = 0; r < runs; r++) for (let n = 0; n < table[2 * r] * size; n++, k++) out[k] = short[k] + table[2 * r + 1];
        return out;
    };
    const triangle = items(triangles, triangleRuns, 3), step = items(steps, stepRuns, 4), region = take(REGION_WORDS);
    if (region.length !== REGION_WORDS) throw new Error('a ground plan mesh ends early');
    return { x, y, corners: corner, triangles: triangle, steps: step, sunk: sunkOf(region) };
}

/** The grid points whose region cells all lie in R: cells ⌊(i − 1) / 2⌋ to ⌊i / 2⌋ across, the same down. */
function sunkOf(region: Uint32Array): Uint32Array<ArrayBuffer> {
    const inR = (cx: number, cy: number) => {
        const bit = (cy + 1) * (CELLS + 2) + cx + 1;
        return ((region[bit >> 5] >>> (bit & 31)) & 1) === 1;
    };
    const out = new Uint32Array(SUNK_WORDS);
    for (let j = 0; j < SIDE; j++) for (let i = 0; i < SIDE; i++) {
        let all = true;
        for (let cy = Math.floor((j - 1) / 2); cy <= j >> 1 && all; cy++) for (let cx = Math.floor((i - 1) / 2); cx <= i >> 1 && all; cx++) all = inR(cx, cy);
        if (all) out[(j * SIDE + i) >> 5] |= 1 << ((j * SIDE + i) & 31);
    }
    return out;
}
