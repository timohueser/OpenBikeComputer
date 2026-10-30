// Feature tiles decoded per specs/trailview-features.md: a string table and a stream per field, into
// features, each a kind with tags and parts in tile units, and a height per vertex once a solver has set one.

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
/** The archive format of specs/trailview-features.md that this decoder reads. */
export const FORMAT = 3;
/** A feature with an alignment counts its coordinates in FINE parts of a tile unit. */
const FINE = 4;
/** The codes of a turn of a tangent, and of a rise per metre in a grade and a bank. */
const TURN = 1024, GRADE = 250, BANK = 1000;
/** The bits of `side`: the way back along a part, and the way on. */
export const SIDE_BACK = 1, SIDE_ON = 2;

/**
 * Decoded bytes a worker keeps: four of the largest zoom 13 tile of the box bake, 279 kB of gzip
 * that decodes to about 8 MB. The four quarters of a tile reach a worker far apart in its queue.
 */
const BUDGET = 32 * 2 ** 20;
const kept = new Map<string, { features: Promise<Feature[]>; bytes: number }>();
let held = 0;

/** The features of tile body `body`, gzip or not, named `name`: decoded once while it is among the last BUDGET bytes. */
export function decodedTile(name: string, body: ArrayBuffer): Promise<Feature[]> {
    let entry = kept.get(name);
    if (entry) kept.delete(name);
    else {
        // The Promise is kept at once, so a second message for the tile waits for this decode.
        const created = entry = { features: decodeBody(body), bytes: 0 };
        created.features.then(features => {
            if (kept.get(name) !== created) return;
            created.bytes = decodedBytes(features);
            held += created.bytes;
            for (const [old, item] of kept) {
                if (held <= BUDGET) break;
                if (item === created) continue;
                kept.delete(old);
                held -= item.bytes;
            }
        }, () => { if (kept.get(name) === created) kept.delete(name); });
    }
    kept.set(name, entry);
    return entry.features;
}

async function decodeBody(body: ArrayBuffer): Promise<Feature[]> {
    let bytes = new Uint8Array(body);
    // A tile service may hand over the gzip of the archive as it is.
    if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
    return decodeFeatures(bytes);
}

/** The heap that `features` hold: their vertices and a cost per feature and per part fitted to the V8 heap of decoded fixture tiles. */
function decodedBytes(features: Feature[]): number {
    let bytes = 0;
    for (const feature of features) {
        bytes += 300;
        for (const part of feature.parts) bytes += 400 + part.xy.byteLength + (part.z?.byteLength ?? 0) + 5 * (part.plan?.tangent.byteLength ?? 0);
    }
    return bytes;
}

/** The curve of a line per vertex, in the direction of the line (specs/trailview-features.md, Alignment). */
export interface Alignment {
    /** Metres along its stroke at the first vertex. */
    along: number;
    /** The metres along fall along the line: it runs against its stroke. */
    backward: boolean;
    /** Radians from x to y. */
    tangent: Float32Array;
    /** Rise per metre. */
    grade: Float32Array;
    /** Rise per metre to the left. */
    bank: Float32Array;
    /** Metres that the half-width gains. */
    widen: Float32Array;
    /** Which ways out of a vertex leave its junction as a side: SIDE_BACK and SIDE_ON. A tile always holds it. */
    side?: Float32Array;
}

export interface FeaturePart {
    /** An outer ring. The rings after it, up to the next outer ring, are its holes. Never set for a point or a line. */
    outer: boolean;
    /** x, y of each vertex in tile units, y down, in quarters of them in a feature with an alignment. A ring does not repeat its first vertex. */
    xy: Int32Array | Float32Array;
    /** Metres per vertex, NaN where no solver has set a height; null when the feature has no height. */
    z: Float32Array | null;
    /** Null for a part that the road solver left alone, and for every part of a tile below zoom 13. */
    plan?: Alignment | null;
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
    const heads = Array.from({ length: read() }, () => ({ kind: string(), header: read() }));
    const features: Feature[] = heads.map(({ kind, header }) => {
        const tags: [string, string][] = [];
        for (let pairs = read(); pairs > 0; pairs--) tags.push([string(), string()]);
        const geometry = GEOMETRIES[header % 4];
        if (!geometry) throw new Error('A feature has an unknown geometry.');
        // fromEntries keeps a key such as `__proto__` as an own property.
        return { kind, tags: Object.fromEntries(tags), geometry, parts: [] };
    });
    heads.forEach(({ header }, f) => {
        const solved = Math.floor(header / 4) % 2 === 1, planned = Math.floor(header / 8) % 2 === 1;
        for (let remaining = Math.floor(header / 16); remaining > 0; remaining--) {
            const head = read(), vertices = Math.floor(head / 2);
            features[f].parts.push({ outer: head % 2 === 1, xy: planned ? new Float32Array(2 * vertices) : new Int32Array(2 * vertices), z: solved ? new Float32Array(vertices) : null, plan: null });
        }
    });
    for (const [f, { parts }] of features.entries()) {
        const fine = Math.floor(heads[f].header / 8) % 2 === 1 ? FINE : 1;
        let x = 0, y = 0, height = 0;
        for (const { xy, z } of parts) {
            for (let v = 0; v < xy.length / 2; v++) {
                xy[2 * v] = (x += signed(read())) / fine;
                xy[2 * v + 1] = (y += signed(read())) / fine;
                if (!z) continue;
                const code = read();
                if (code) height += signed(code - 1);
                z[v] = code ? height / 100 : NaN;
            }
        }
    }
    const aligned: { part: FeaturePart; code: number }[] = [];
    for (const [f, { parts }] of features.entries()) {
        if (Math.floor(heads[f].header / 8) % 2 === 0) continue;
        for (const part of parts) {
            const code = read();
            if (code) aligned.push({ part, code });
        }
    }
    /** The values of a stream of the alignment, per part with one, each the sum of the codes of its part so far when `running`. */
    const column = (decoded: (value: number) => number, running = false) => aligned.map(({ part }) => {
        const values = new Float32Array(part.xy.length / 2);
        let sum = 0;
        for (let v = 0; v < values.length; v++) values[v] = decoded(running ? (sum += signed(read())) : read());
        return values;
    });
    const tangents = column(value => ((value % TURN) + TURN) % TURN * (2 * Math.PI / TURN), true), grades = column(value => value / GRADE, true);
    const banks = column(value => signed(value) / BANK), words = column(value => value);
    aligned.forEach(({ part, code }, n) => {
        part.plan = {
            along: Math.floor((code - 1) / 2) / 10, backward: (code - 1) % 2 === 1, tangent: tangents[n], grade: grades[n], bank: banks[n],
            widen: words[n].map(word => Math.floor(word / 4) / 100), side: words[n].map(word => word % 4),
        };
    });
    if (at !== bytes.length) throw new Error('The feature tile has bytes after its last feature.');
    return features;
}
