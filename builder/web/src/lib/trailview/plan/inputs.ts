// The inputs of one cell of the ground plan, collected on the main thread from the tiles that the
// viewer already loads. The bridge names them, so no TypeScript copy of the crate's input walk exists.
import type { FeatureTiles } from '../terrain/stamps';

/** The zoom of the feature tiles that a cell reads: FEATURE_ZOOM of the crate. */
const FEATURE_ZOOM = 13;

/** The body of stored terrain tile z/x/y, undefined where it is absent. */
export type StoredTiles = (z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>;

/** The bridge's names of a cell's inputs; `words` is `words.wanted` of its vocabulary. */
export interface InputNames {
    feature_tiles(x: number, y: number): Uint32Array;
    wanted(x: number, y: number): Uint32Array;
    words: number;
}

export interface CellInputs {
    /** The feature tiles that exist, as bodies of the archive: they can be gzip. */
    tiles: { column: number; row: number; body: ArrayBuffer }[];
    /** The stored tiles that hold the wanted pages and exist, each once. */
    terrain: { z: number; x: number; y: number; body: ArrayBuffer }[];
    /** The bridge's `wanted` of the cell. */
    pages: Uint32Array;
}

/**
 * The inputs of cell (x, y) of zoom 16, or 'empty' when none of its feature tiles exists. A fetch that
 * fails rejects: a cell never builds with an input left out, which would differ from the native build.
 */
export async function cellInputs({ x, y }: { x: number; y: number }, names: InputNames, features: FeatureTiles, stored: StoredTiles): Promise<CellInputs | 'empty'> {
    const pairs = names.feature_tiles(x, y);
    const found = await Promise.all(Array.from({ length: pairs.length / 2 }, async (_, i) => {
        const [column, row] = [pairs[2 * i], pairs[2 * i + 1]];
        return { column, row, body: await features(FEATURE_ZOOM, column, row) };
    }));
    const tiles = found.flatMap(({ column, row, body }) => (body ? [{ column, row, body }] : []));
    if (!tiles.length) return 'empty';
    const pages = names.wanted(x, y), seen = new Map<string, [number, number, number]>();
    for (let i = 0; i < pages.length; i += names.words) seen.set(`${pages[i + 3]}/${pages[i + 4]}/${pages[i + 5]}`, [pages[i + 3], pages[i + 4], pages[i + 5]]);
    const bodies = await Promise.all([...seen.values()].map(async ([z, tx, ty]) => ({ z, x: tx, y: ty, body: await stored(z, tx, ty) })));
    return { tiles, terrain: bodies.flatMap(({ body, ...at }) => (body ? [{ ...at, body }] : [])), pages };
}
