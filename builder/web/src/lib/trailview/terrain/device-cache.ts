// The device cache (`ground=plan`): what a worker makes of the release's tiles, kept in Cache Storage under
// the release and a build version, the hash of the code that makes it (model.ts: the plan's cells, stamps.ts:
// the stamp worker's nodes). That code is a pure function of the tiles, so an entry is byte-equal to what the
// worker makes. The least recently used entries leave past the cap.

/**
 * The device storage of each kind: past its cap, the least recently used entries of the kind go. Together
 * 256 MiB; a view writes more cell bytes than stamp bytes.
 */
export const CAPS = { cells: 192 * 2 ** 20, stamps: 64 * 2 ** 20 };
/** The entry's first word, then the count of parts, their lengths in words and the SHA-256 of the parts. */
const MAGIC = 0x4f424343;
const DIGEST_WORDS = 8;
/** The key of the index: each entry's bytes, least recently used first. */
const INDEX = 'https://cache.invalid/index';
/** The index is saved at most this often. */
const SAVE_MS = 2000;

const urlOf = (key: string) => `https://cache.invalid/${key}`;

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

export class DeviceCache {
    private saving?: ReturnType<typeof setTimeout>;
    /** Each entry's bytes, least recently used first. */
    private readonly index = new Map<string, number>();
    /** Settles once the saved index is merged in: a read does not wait for it, a write and a save do. */
    private readonly loaded: Promise<void>;

    private constructor(private readonly cache: Cache, private readonly cap: number) {
        this.loaded = this.load().catch(() => {});
    }

    /**
     * The cache of a kind, a release (any string that names its data) and a build version; undefined where
     * the context has no Cache Storage. A cache of the kind for another release or build goes, in the background.
     */
    static async open(kind: string, release: string, build: string, cap: number): Promise<DeviceCache | undefined> {
        if (!globalThis.caches) return undefined;
        const prefix = `trailview-${kind}/`, name = `${prefix}${await sha256(release)}/${build}`;
        void caches.keys().then(names => Promise.all(names.filter(other => other.startsWith(prefix) && other !== name).map(other => caches.delete(other)))).catch(() => {});
        return new DeviceCache(await caches.open(name), cap);
    }

    /**
     * Merges the saved index in before the entries that this session used. Entries that it does not know
     * count as least recently used, with their bytes unknown until a read: the first to go past the cap.
     */
    private async load(): Promise<void> {
        const [saved, requests] = await Promise.all([this.cache.match(INDEX).then(r => r?.json()).catch(() => undefined) as Promise<Record<string, number> | undefined>, this.cache.keys()]);
        const urls = new Set(requests.map(request => request.url).filter(url => url !== INDEX)), used = [...this.index];
        this.index.clear();
        for (const url of urls) if (!saved || !Object.hasOwn(saved, url)) this.index.set(url, 0);
        for (const [url, bytes] of Object.entries(saved ?? {})) if (urls.has(url)) this.index.set(url, bytes);
        for (const [url, bytes] of used) {
            this.index.delete(url);
            this.index.set(url, bytes);
        }
    }

    /** The parts of an entry; a partial or corrupt entry goes and reads as missing. */
    async read(key: string): Promise<Uint32Array[] | undefined> {
        const url = urlOf(key), response = await this.cache.match(url);
        if (!response) return undefined;
        const parts = await decode(await response.arrayBuffer());
        this.index.delete(url);
        if (parts) this.index.set(url, Number(response.headers.get('x-bytes')) || 0);
        else await this.cache.delete(url);
        this.save();
        return parts;
    }

    /** Keeps an entry; the least recently used entries go until all fit the cap. A full storage drops the entry. */
    async write(key: string, parts: Uint32Array[]): Promise<void> {
        const url = urlOf(key), blob = await encode(parts);
        if (blob.size > this.cap) return;
        await this.cache.put(url, new Response(blob, { headers: { 'x-bytes': String(blob.size) } }));
        await this.loaded;
        this.index.delete(url);
        this.index.set(url, blob.size);
        for (const old of this.index.keys()) {
            if (this.bytes <= this.cap) break;
            this.index.delete(old);
            await this.cache.delete(old);
        }
        this.save();
    }

    /** The bytes of all entries that it knows the size of. */
    get bytes(): number {
        let sum = 0;
        for (const bytes of this.index.values()) sum += bytes;
        return sum;
    }

    private save(): void {
        this.saving ??= setTimeout(() => {
            this.saving = undefined;
            void this.loaded.then(() => this.cache.put(INDEX, Response.json(Object.fromEntries(this.index)))).catch(() => {});
        }, SAVE_MS);
    }
}
