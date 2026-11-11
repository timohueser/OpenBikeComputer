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
export const FORMAT = 4;
/** A feature with an alignment counts its coordinates in FINE parts of a tile unit. */
const FINE = 4;
/** The codes of a turn of a tangent, and of a rise per metre in a grade and a bank. */
const TURN = 1024, GRADE = 250, BANK = 1000;
/** The bits of `side`: the way back along a part, and the way on. */
export const SIDE_BACK = 1, SIDE_ON = 2;
/** The bits of `walk`: a sidewalk left and right of the direction of a part. */
export const WALK_LEFT = 1, WALK_RIGHT = 2;

/**
 * Decoded bytes a worker keeps: four of the largest zoom 13 tile of the box bake, 279 kB of gzip
 * that decodes to about 8 MB. The four quarters of a tile reach a worker far apart in its queue.
 */
let budget = 32 * 2 ** 20;
const kept = new Map<string, { features: Promise<Feature[]>; bytes: number; charged: number; pinned?: boolean }>();
let held = 0;

/** Sets the bytes that this worker keeps, for a worker that hangs more than decoded features on its tiles. */
export function tileBudget(bytes: number): void {
    budget = bytes;
}

/** The tiles that jobs on their way or still wished read: see wantTiles. */
let wanted = new Set<string>();

/** Names the tiles that jobs on their way or still wished read: `trim` keeps them, as it keeps the newest. The others fall back under the budget at once. */
export function wantTiles(names: Iterable<string>): void {
    wanted = new Set(names);
    trim();
}

/**
 * The tiles that have gone longest without use go, but never `newest` or a wanted one: an unpinned one past the
 * budget, a pinned one past a third of it, so the unpinned tiles always keep the rest.
 */
function trim(newest?: { bytes: number; charged: number }): void {
    let pins = 0;
    for (const item of kept.values()) if (item.pinned) pins += item.bytes + item.charged;
    for (const [old, item] of kept) {
        if (held <= budget && pins <= budget / 3) break;
        if (item === newest || wanted.has(old) || (item.pinned ? pins <= budget / 3 : held <= budget)) continue;
        kept.delete(old);
        held -= item.bytes + item.charged;
        if (item.pinned) pins -= item.bytes + item.charged;
    }
}

/** Keeps a kept tile through `trim` while `pinned`, within a third of the budget. A failed decode leaves `kept` at once, pinned or not. */
export function pinTile(name: string, pinned: boolean): void {
    const entry = kept.get(name);
    if (entry) entry.pinned = pinned;
}

/** Counts `bytes` that hang from the features of a kept tile, in place of what was counted for it before. */
export function chargeTile(name: string, bytes: number): void {
    const entry = kept.get(name);
    if (!entry) return;
    held += bytes - entry.charged;
    entry.charged = bytes;
    trim(entry);
}

/** The features of tile `name` while it is among the last bytes of the budget; undefined after it left. */
export function heldTile(name: string): Promise<Feature[]> | undefined {
    const entry = kept.get(name);
    if (!entry) return undefined;
    kept.delete(name);
    kept.set(name, entry);
    return entry.features;
}

/** The features of tile body `body`, gzip or not, named `name`: decoded once while it is among the last bytes of the budget. */
export function decodedTile(name: string, body: ArrayBuffer): Promise<Feature[]> {
    const known = heldTile(name);
    if (known) return known;
    // The Promise is kept at once, so a second message for the tile waits for this decode.
    const created = { features: decodeBody(body), bytes: 0, charged: 0 };
    created.features.then(features => {
        if (kept.get(name) !== created) return;
        created.bytes = decodedBytes(features);
        held += created.bytes;
        trim(created);
    }, () => { if (kept.get(name) === created) kept.delete(name); });
    kept.set(name, created);
    return created.features;
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
    /** Which ways out of a vertex leave its junction as a side: SIDE_BACK and SIDE_ON. */
    side: Float32Array;
    /** The sides of the segment from a vertex on that have a sidewalk: WALK_LEFT and WALK_RIGHT. */
    walk: Float32Array;
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
            widen: words[n].map(word => Math.floor(word / 16) / 100), side: words[n].map(word => word % 4), walk: words[n].map(word => Math.floor(word / 4) % 4),
        };
    });
    if (at !== bytes.length) throw new Error('The feature tile has bytes after its last feature.');
    return features;
}

/**
 * PROTOTYPE (crossing look study, never merged): the bytes of a feature tile with other heights. `offset` gives the
 * metres to add at a vertex of a feature of `kind` at world units x, y; the grades of a moved line follow. Every other
 * byte stays. The decoder above is the format's reference: this walks the same streams.
 */
export function movedHeights(bytes: Uint8Array, tile: { level: number; x: number; y: number }, offset: (kind: string, x: number, y: number) => number): Uint8Array {
    let at = 0;
    const read = (): number => {
        let value = 0, scale = 1, byte: number;
        do {
            byte = bytes[at++];
            value += (byte & 0x7f) * scale;
            scale *= 128;
        } while (byte & 0x80);
        return value;
    };
    const signed = (value: number) => (value % 2 ? -(value + 1) / 2 : value / 2), zigzag = (n: number) => (n < 0 ? -2 * n - 1 : 2 * n);
    const out: number[] = [];
    const write = (value: number) => {
        do {
            const low = value % 128;
            value = Math.floor(value / 128);
            out.push(value ? low | 0x80 : low);
        } while (value);
    };
    const copy = (from: number, to: number) => { for (let i = from; i < to; i++) out.push(bytes[i]); };
    const text = new TextDecoder(), strings: string[] = [];
    for (let count = read(); count > 0; count--) {
        const length = read();
        strings.push(text.decode(bytes.subarray(at, at + length)));
        at += length;
    }
    const heads = Array.from({ length: read() }, () => ({ kind: strings[read()], header: read() }));
    for (let f = 0; f < heads.length; f++) for (let pairs = read(); pairs > 0; pairs--) { read(); read(); }
    const counts = heads.map(({ header }) => Array.from({ length: Math.floor(header / 16) }, () => Math.floor(read() / 2)));
    copy(0, at);
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT, metres = 2 * Math.PI * 6378137 / 2 ** 30 / Math.cosh(Math.PI * (1 - 2 * (oy * scale) / 2 ** 30));
    /** Per part of a moved feature: the metres added at each vertex, and the metres along the part there. */
    const moved = new Map<string, { add: number[]; along: number[] }>();
    for (const [f, { kind, header }] of heads.entries()) {
        const solved = Math.floor(header / 4) % 2 === 1, fine = Math.floor(header / 8) % 2 === 1 ? FINE : 1, from = at;
        let x = 0, y = 0, height = 0, any = false;
        const parts = counts[f].map(vertices => {
            const raw: number[] = [], z: number[] = [], add: number[] = [], along: number[] = [];
            let px = 0, py = 0;
            for (let v = 0; v < vertices; v++) {
                const dx = read(), dy = read();
                x += signed(dx);
                y += signed(dy);
                raw.push(dx, dy);
                const wx = (ox + x / fine) * scale, wy = (oy + y / fine) * scale;
                along.push(v ? along[v - 1] + Math.hypot(wx - px, wy - py) * metres : 0);
                [px, py] = [wx, wy];
                if (!solved) continue;
                const code = read();
                if (code) height += signed(code - 1);
                z.push(code ? height : NaN);
                add.push(code ? offset(kind, wx, wy) : 0);
                any ||= add[v] !== 0;
            }
            return { raw, z, add, along };
        });
        if (!any) {
            copy(from, at);
            continue;
        }
        let now = 0;
        parts.forEach(({ raw, z, add, along }, p) => {
            moved.set(`${f}/${p}`, { add, along });
            for (let v = 0; v < z.length; v++) {
                write(raw[2 * v]);
                write(raw[2 * v + 1]);
                if (Number.isNaN(z[v])) {
                    write(0);
                    continue;
                }
                const cm = z[v] + Math.round(100 * add[v]);
                write(1 + zigzag(cm - now));
                now = cm;
            }
        });
    }
    // The alignments: which parts have one, then the tangents, the grades, the banks and the words, a stream each.
    const aligned: { key: string; vertices: number }[] = [], start = at;
    for (const [f, { header }] of heads.entries()) {
        if (Math.floor(header / 8) % 2 === 0) continue;
        counts[f].forEach((vertices, p) => { if (read()) aligned.push({ key: `${f}/${p}`, vertices }); });
    }
    for (const { vertices } of aligned) for (let v = 0; v < vertices; v++) read();
    copy(start, at);
    for (const { key, vertices } of aligned) {
        const from = at, part = moved.get(key), grades: number[] = [];
        let sum = 0;
        for (let v = 0; v < vertices; v++) grades.push((sum += signed(read())));
        if (!part) {
            copy(from, at);
            continue;
        }
        let now = 0;
        for (let v = 0; v < vertices; v++) {
            const a = Math.max(v - 1, 0), b = Math.min(v + 1, vertices - 1), run = part.along[b] - part.along[a];
            const grade = grades[v] + Math.round(GRADE * (run > 0 ? (part.add[b] - part.add[a]) / run : 0));
            write(zigzag(grade - now));
            now = grade;
        }
    }
    copy(at, bytes.length);
    return new Uint8Array(out);
}
