// The device cache of built cells (`ground=plan`): each cell's records block and rim, and its mesh, in
// Cache Storage, under the release and the SHA-256 of the bridge's wasm. The wasm embeds the crate and its
// rule tables, and the crate is a pure function of the release's tiles, so a cached cell is byte-equal to a
// built one. The least recently used entries leave past CAP_BYTES.

/** The prefix of the caches' names: a cache of another release or wasm goes when one opens. */
const PREFIX = 'trailview-cells/';
/** The device storage of the cells: about six town views, or 300 km of a planned route. */
export const CAP_BYTES = 256 * 2 ** 20;
/** The entry's first word, then the count of parts, their lengths in words and the SHA-256 of the parts. */
const MAGIC = 0x4f424343;
const DIGEST_WORDS = 8;
/** The key of the index: each entry's bytes, least recently used first. */
const INDEX = 'https://cells.invalid/index';
/** The index is saved at most this often. */
const SAVE_MS = 2000;

export type CellPart = 'cell' | 'mesh';

const urlOf = ({ x, y }: { x: number; y: number }, part: CellPart) => `https://cells.invalid/${x}/${y}/${part}`;

/** The SHA-256 of bytes or a string, as hex. */
export async function sha256(data: BufferSource | string): Promise<string> {
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(b => b.toString(16).padStart(2, '0')).join('');
}

/** An entry: the header, the SHA-256 of the parts and the parts. */
export async function encode(parts: Uint32Array[]): Promise<Blob> {
    const payload = new Uint32Array(parts.reduce((sum, p) => sum + p.length, 0));
    parts.reduce((at, p) => (payload.set(p, at), at + p.length), 0);
    const head = new Uint32Array([MAGIC, parts.length, ...parts.map(p => p.length)]);
    return new Blob([head, await crypto.subtle.digest('SHA-256', payload), payload]);
}

/** The parts of an entry, or undefined when it is partial or corrupt. */
export async function decode(buffer: ArrayBuffer): Promise<Uint32Array[] | undefined> {
    if (buffer.byteLength % 4) return undefined;
    const words = new Uint32Array(buffer);
    if (words.length < 2 || words[0] !== MAGIC) return undefined;
    const n = words[1], first = 2 + n + DIGEST_WORDS;
    if (words.length < first) return undefined;
    const lengths = [...words.subarray(2, 2 + n)];
    if (first + lengths.reduce((a, b) => a + b, 0) !== words.length) return undefined;
    const digest = new Uint32Array(await crypto.subtle.digest('SHA-256', words.subarray(first))), stored = words.subarray(2 + n, first);
    if (!digest.every((w, i) => w === stored[i])) return undefined;
    let at = first;
    // Each part its own buffer: a kept rim does not hold the whole entry.
    return lengths.map(length => words.slice(at, (at += length)));
}

export class CellCache {
    private saving?: ReturnType<typeof setTimeout>;

    /** `index` holds each entry's bytes, least recently used first. */
    private constructor(private readonly cache: Cache, private readonly index: Map<string, number>, private readonly cap: number) {}

    /**
     * The cache of a release (any string that names its data) and a build (the SHA-256 of the bridge's
     * wasm); undefined where the context has no Cache Storage. Entries that the saved index does not know
     * yet count as least recently used.
     */
    static async open(release: string, build: string, cap = CAP_BYTES): Promise<CellCache | undefined> {
        if (!globalThis.caches) return undefined;
        const name = `${PREFIX}${await sha256(release)}/${build}`;
        for (const other of await caches.keys()) if (other.startsWith(PREFIX) && other !== name) await caches.delete(other);
        const cache = await caches.open(name);
        const saved = (await (await cache.match(INDEX))?.json().catch(() => undefined)) as Record<string, number> | undefined;
        const urls = new Set((await cache.keys()).map(request => request.url).filter(url => url !== INDEX));
        const index = new Map<string, number>();
        for (const url of urls) if (!saved || !Object.hasOwn(saved, url)) index.set(url, Number((await cache.match(url))?.headers.get('x-bytes')) || 0);
        for (const [url, bytes] of Object.entries(saved ?? {})) if (urls.has(url)) index.set(url, bytes);
        return new CellCache(cache, index, cap);
    }

    /** The parts of a cell's entry; a partial or corrupt entry goes and reads as missing. */
    async read(cell: { x: number; y: number }, part: CellPart): Promise<Uint32Array[] | undefined> {
        const url = urlOf(cell, part), response = await this.cache.match(url);
        if (!response) return undefined;
        const parts = await decode(await response.arrayBuffer()), bytes = this.index.get(url) ?? 0;
        this.index.delete(url);
        if (parts) this.index.set(url, bytes);
        else await this.cache.delete(url);
        this.save();
        return parts;
    }

    /** Keeps a cell's entry; the least recently used entries go until all fit the cap. A full storage drops the entry. */
    async write(cell: { x: number; y: number }, part: CellPart, parts: Uint32Array[]): Promise<void> {
        const url = urlOf(cell, part), blob = await encode(parts);
        if (blob.size > this.cap) return;
        await this.cache.put(url, new Response(blob, { headers: { 'x-bytes': String(blob.size) } }));
        this.index.delete(url);
        this.index.set(url, blob.size);
        for (const old of this.index.keys()) {
            if (this.bytes <= this.cap) break;
            this.index.delete(old);
            await this.cache.delete(old);
        }
        this.save();
    }

    /** The bytes of all entries. */
    get bytes(): number {
        let sum = 0;
        for (const bytes of this.index.values()) sum += bytes;
        return sum;
    }

    private save(): void {
        this.saving ??= setTimeout(() => {
            this.saving = undefined;
            void this.cache.put(INDEX, Response.json(Object.fromEntries(this.index))).catch(() => {});
        }, SAVE_MS);
    }
}
