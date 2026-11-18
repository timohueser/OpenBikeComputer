// A cell's near mesh as the plan worker hands it over (`CellMesh::to_bytes` of `mesh/mod.rs` in
// obc-ground-plan), then the meshes of its storeys over the ground (`StoreyMesh::to_bytes` of `mesh/shell.rs`)
// where it has any: the one reader of those bytes, in the form that pieces.ts uploads.

/** A cell mesh (`MAGIC` of `mesh/mod.rs`): its head ends with the count of the end faces on a portal line. */
const MAGIC = 0x4f424d06;
const HEAD = 13;
/** A storey mesh (`MAGIC` of `mesh/shell.rs`). */
const STOREY_MAGIC = 0x4f425303;
/**
 * The storey mesh's head: its magic, cell, counts and top slots, then the triangles of each part (`PARTS`), the slots of each
 * kind after the tops' (`SLOT_KINDS`), the count of the caps and the count of the lights.
 */
const STOREY_HEAD = 9;
const DECK_PARTS = 7;
const PARTS = DECK_PARTS + 4;
const SLOT_KINDS = 4;
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
    /** Three 16-bit corner indices per triangle, the strip's among them, each less the base of its run. */
    triangles: Uint16Array<ArrayBuffer>;
    /** The runs of triangles: their count and the base that their indices add. */
    runs: { count: number; base: number }[];
    /** The last runs, which draw only over level-18 nodes: the strip's triangles of halves that the level-17 terrain draws. */
    level18Runs: number;
    /** Four corner indices per step slot: a_p, a_q, b_q, b_p. */
    steps: Uint32Array<ArrayBuffer>;
    /** The last slots of `steps`: the end faces on the deck end lines, and last of them those beside an opening on a portal line. */
    endFaces: number;
    headwalls: number;
    /** One bit per grid point (j × SIDE + i) whose region cells all lie in R: the terrain sinks there. */
    sunk: Uint32Array<ArrayBuffer>;
    /** Four words per culvert mouth: its corner on the water, its corner in the zone, the outline's direction and the water's half-width. */
    culverts: Uint32Array<ArrayBuffer>;
    /** The meshes of the cell's storeys over the ground, where it has any. */
    storeys?: StoreyMesh;
}

/**
 * The storeys' meshes of a cell: the sheets and the shells about them, and the bores. A corner's height is that of its label plus
 * its offset, or where it has caps, the least of that and each cap's; the top's triangles come first, then the shell's
 * (underside, edges, parapets, railings, posts, piers), then the bores' (floor, roof, walls, fronts), and where daylight comes
 * into the bores.
 */
export interface StoreyMesh {
    /** Two words per corner, as the cell mesh's (never a terrain vertex), and each corner's offset in metres (f32 bits). */
    corners: Uint32Array<ArrayBuffer>;
    offsets: Uint32Array<ArrayBuffer>;
    /** Three whole corner indices per triangle; the count of the top's, and the first of the railings', the piers', the bores' (their floors), their roofs' and their fronts'. */
    triangles: Uint32Array<ArrayBuffer>;
    top: number;
    railings: number;
    piers: number;
    bores: number;
    roofs: number;
    fronts: number;
    /** Four whole corner indices per slot (a_p, a_q, b_q, b_p): the tops' steps between two labels, the undersides' risers, then the bores' seams and sills; the first of the seams and of the sills. */
    slots: Uint32Array<ArrayBuffer>;
    seams: number;
    sills: number;
    /** Three words per cap, sorted by corner: the corner, the cap's label (NO_LABEL: the plain terrain) and its offset (f32 bits). */
    caps: Uint32Array<ArrayBuffer>;
    /** Three words where daylight comes into a bore way: the way (its records line's first word), its metres along there and the metres to the nearest portal (f32 bits). */
    lights: Uint32Array<ArrayBuffer>;
}

/** The bytes that `cellMesh` reads: a cell's mesh, then its storeys' mesh where it has a storey corner. */
export function meshBytes(mesh: Uint8Array, storeys?: Uint8Array): ArrayBuffer {
    // A storey mesh's corner count follows its magic and its cell.
    if (!storeys || !new Uint32Array(storeys.buffer, storeys.byteOffset, 4)[3]) {
        return mesh.byteOffset === 0 && mesh.byteLength === mesh.buffer.byteLength ? (mesh.buffer as ArrayBuffer) : mesh.slice().buffer;
    }
    const out = new Uint8Array(mesh.byteLength + storeys.byteLength);
    out.set(mesh);
    out.set(storeys, mesh.byteLength);
    return out.buffer;
}

export function cellMesh(bytes: ArrayBuffer): CellMesh {
    const words = new Uint32Array(bytes);
    const [magic, x, y, corners, triangles, , triangleRuns, steps, stepRuns, level18Runs, endFaces, mouths, headwalls] = words.subarray(0, HEAD);
    if (magic !== MAGIC) throw new Error(`not a ground plan mesh: ${magic.toString(16)}`);
    let at = HEAD;
    const take = (n: number) => words.subarray(at, (at += n));
    const corner = words.slice(at, (at += 2 * corners));
    /** Items of `size` corners in runs of (count, base), their 16-bit indices less the base. */
    const items = (count: number, runs: number, size: number) => {
        // Whole words, so that the indices upload as they are.
        const table = take(2 * runs), short = new Uint16Array(bytes.slice(4 * at, 4 * (at += Math.ceil((size * count) / 2))), 0, size * count);
        return { short, runs: Array.from({ length: runs }, (_, r) => ({ count: table[2 * r], base: table[2 * r + 1] })) };
    };
    const triangle = items(triangles, triangleRuns, 3), step = items(steps, stepRuns, 4), region = take(REGION_WORDS), culverts = take(4 * mouths).slice();
    if (region.length !== REGION_WORDS || culverts.length !== 4 * mouths) throw new Error('a ground plan mesh ends early');
    const storeys = at < words.length ? storeyMesh(bytes, at) : undefined;
    return { x, y, corners: corner, triangles: triangle.short, runs: triangle.runs, level18Runs, steps: whole(step, 4), endFaces, headwalls, sunk: sunkOf(region), culverts, storeys };
}

/** Items of runs as whole corner indices: the vertex stage reads them by index. */
function whole({ short, runs }: { short: Uint16Array; runs: { count: number; base: number }[] }, size: number): Uint32Array<ArrayBuffer> {
    const out = new Uint32Array(short.length);
    let k = 0;
    for (const { count, base } of runs) for (let n = 0; n < size * count; n++, k++) out[k] = short[k] + base;
    return out;
}

/** The storey meshes from word `start` of `bytes` on. */
function storeyMesh(bytes: ArrayBuffer, start: number): StoreyMesh {
    const words = new Uint32Array(bytes, 4 * start);
    const [magic, , , corners, triangles, triangleRuns, slots, slotRuns, topSlots] = words.subarray(0, STOREY_HEAD);
    if (magic !== STOREY_MAGIC) throw new Error(`not a storey mesh: ${magic.toString(16)}`);
    let at = STOREY_HEAD;
    const take = (n: number) => words.subarray(at, (at += n));
    const parts = take(PARTS), kinds = take(SLOT_KINDS), [caps, lights] = take(2);
    const corner = take(3 * corners), packed = new Uint32Array(2 * corners), offsets = new Uint32Array(corners);
    for (let k = 0; k < corners; k++) {
        packed[2 * k] = corner[3 * k];
        packed[2 * k + 1] = corner[3 * k + 1];
        offsets[k] = corner[3 * k + 2];
    }
    const items = (count: number, runs: number, size: number) => {
        const table = take(2 * runs), short = new Uint16Array(bytes.slice(4 * (start + at), 4 * (start + (at += Math.ceil((size * count) / 2))))).subarray(0, size * count);
        return { short, runs: Array.from({ length: runs }, (_, r) => ({ count: table[2 * r], base: table[2 * r + 1] })) };
    };
    const triangle = items(triangles, triangleRuns, 3), slot = items(slots, slotRuns, 4), cap = take(3 * caps).slice(), light = take(3 * lights).slice();
    if (at > words.length || cap.length !== 3 * caps || light.length !== 3 * lights) throw new Error('a storey mesh ends early');
    // The parts' triangles: top, underside, edge, parapet, railing, post, pier, then the bores' floor, roof, wall, front. The slots:
    // the tops', the risers, the floors' and the roofs' seams, the sills.
    const first = (k: number) => parts.subarray(0, k).reduce((sum, n) => sum + n, 0), seams = topSlots + kinds[0];
    return {
        corners: packed, offsets, triangles: whole(triangle, 3), top: parts[0], railings: first(4), piers: first(6), bores: first(DECK_PARTS),
        roofs: first(DECK_PARTS + 1), fronts: first(PARTS - 1), slots: whole(slot, 4), seams, sills: seams + kinds[1] + kinds[2], caps: cap, lights: light,
    };
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
