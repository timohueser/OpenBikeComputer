// Feature tiles decoded per specs/trailview-features.md: a string table, then features, each a
// kind with tags and parts in tile units, and a height per vertex once a solver has set one.

export type Geometry = 'point' | 'line' | 'area';

/** Tile units across a tile. The buffer lies outside 0..EXTENT. */
export const EXTENT = 16384;
/** The zoom whose tiles hold every kind, at full detail. */
export const DEEPEST = 13;
/** World units (2^30 across the world) per tile unit of a tile of zoom `level`. */
export const unitScale = (level: number) => 2 ** (30 - level) / EXTENT;
/** The square of tile `part`, at or under tile `tile`, in the tile units of `tile`: its west and north edges and its size. */
export function square(tile: { level: number; x: number; y: number }, part: { level: number; x: number; y: number }): [number, number, number] {
    const size = EXTENT / 2 ** (part.level - tile.level);
    return [part.x * size - tile.x * EXTENT, part.y * size - tile.y * EXTENT, size];
}
/** Decoded tiles a worker keeps. */
const KEPT = 24;
const kept = new Map<string, Feature[]>();

/** The features of tile body `body`, gzip or not, named `name`: decoded once while it is among the last KEPT. */
export async function decodedTile(name: string, body: ArrayBuffer): Promise<Feature[]> {
    let features = kept.get(name);
    if (features) {
        kept.delete(name);
    } else {
        let bytes = new Uint8Array(body);
        // A tile service may hand over the gzip of the archive as it is.
        if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
        features = decodeFeatures(bytes);
    }
    kept.set(name, features);
    if (kept.size > KEPT) kept.delete(kept.keys().next().value!);
    return features;
}

export interface FeaturePart {
    /** An outer ring. The rings after it, up to the next outer ring, are its holes. Never set for a point or a line. */
    outer: boolean;
    /** x, y of each vertex in tile units, y down. A ring does not repeat its first vertex. */
    xy: Int32Array;
    /** Metres per vertex, NaN where no solver has set a height; null when the feature has no height. */
    z: Float32Array | null;
}

export interface Feature {
    kind: string;
    tags: Record<string, string>;
    geometry: Geometry;
    parts: FeaturePart[];
}

const GEOMETRIES: Geometry[] = ['point', 'line', 'area'];

/** The features of one decompressed tile. */
export function decodeFeatures(bytes: Uint8Array): Feature[] {
    let at = 0;
    // Arithmetic, not bit operations: a varint can hold more than 31 bits.
    const read = (): number => {
        let value = 0, scale = 1, byte: number;
        do {
            if (at >= bytes.length) throw new Error('The feature tile ends inside a number.');
            byte = bytes[at++];
            value += (byte & 0x7f) * scale;
            scale *= 128;
        } while (byte & 0x80);
        return value;
    };
    const signed = (value: number) => (value % 2 ? -(value + 1) / 2 : value / 2);
    const text = new TextDecoder('utf-8', { fatal: true });
    const strings: string[] = [];
    for (let count = read(); count > 0; count--) {
        const length = read();
        if (at + length > bytes.length) throw new Error('The feature tile ends inside a string.');
        strings.push(text.decode(bytes.subarray(at, at + length)));
        at += length;
    }
    const string = (): string => {
        const value = strings[read()];
        if (value === undefined) throw new Error('A feature names a string that the tile does not have.');
        return value;
    };
    const features: Feature[] = [];
    for (let count = read(); count > 0; count--) {
        const kind = string();
        const tags: [string, string][] = [];
        for (let pairs = read(); pairs > 0; pairs--) tags.push([string(), string()]);
        const header = read();
        const geometry = GEOMETRIES[header % 4];
        if (!geometry) throw new Error('A feature has an unknown geometry.');
        const solved = Math.floor(header / 4) % 2 === 1;
        const parts: FeaturePart[] = [];
        let x = 0, y = 0, height = 0;
        for (let remaining = Math.floor(header / 8); remaining > 0; remaining--) {
            const head = read(), vertices = Math.floor(head / 2);
            const xy = new Int32Array(2 * vertices), z = solved ? new Float32Array(vertices) : null;
            for (let v = 0; v < vertices; v++) {
                xy[2 * v] = x += signed(read());
                xy[2 * v + 1] = y += signed(read());
                if (!z) continue;
                const code = read();
                if (code) height += signed(code - 1);
                z[v] = code ? height / 100 : NaN;
            }
            parts.push({ outer: head % 2 === 1, xy, z });
        }
        // fromEntries keeps a key such as `__proto__` as an own property.
        features.push({ kind, tags: Object.fromEntries(tags), geometry, parts });
    }
    if (at !== bytes.length) throw new Error('The feature tile has bytes after its last feature.');
    return features;
}
