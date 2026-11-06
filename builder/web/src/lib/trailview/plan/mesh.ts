// A cell's near mesh as the plan worker hands it over (`CellMesh::to_bytes` of `mesh/mod.rs` in
// obc-ground-plan): the one reader of those bytes, in the form that pieces.ts uploads.

const MAGIC = 0x4f424d01;
const HEAD = 7;
/** Level-18 grid points along a side of a cell, both borders included, and the words of one bit each. */
export const SIDE = 257;
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
    /** One bit per level-18 grid point (j × SIDE + i) strictly inside the owned region: the terrain sinks there. */
    sunk: Uint32Array<ArrayBuffer>;
}

export function cellMesh(bytes: ArrayBuffer): CellMesh {
    const words = new Uint32Array(bytes);
    const [magic, x, y, corners, triangles, , steps] = words.subarray(0, HEAD);
    if (magic !== MAGIC) throw new Error(`not a ground plan mesh: ${magic.toString(16)}`);
    let at = HEAD;
    const take = (n: number) => words.slice(at, (at += n));
    const mesh = { x, y, corners: take(2 * corners), triangles: take(3 * triangles), steps: take(4 * steps), sunk: take(SUNK_WORDS) };
    if (mesh.sunk.length !== SUNK_WORDS) throw new Error('a ground plan mesh ends early');
    return mesh;
}
