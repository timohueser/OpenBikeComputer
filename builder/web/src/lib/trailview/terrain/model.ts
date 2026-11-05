// The one road model (prototype, `ground=model`): the records of GP3's candidates per zoom-16 cell, made
// by the crate's `debug model` for the fixture and served as one file beside the feature tiles. The look
// evaluates them per fragment (model.wgsl). Not for merge.

/** Words per row of the texture that holds the records: model.wgsl's MODEL_ROW. */
export const MODEL_ROW = 4096;

/**
 * The settings of a device tier (`tier=low` in the URL, else high): whether the look draws the roads past the masks
 * (`farroads=0|1` overrides it), and whether it keeps the look's caches, of the mask pages and of the drawn pages past
 * them, with the records where a cache cannot prove a pixel (probe bits 2097152 and 33554432; the far cache needs the
 * page cache's texture).
 */
export const TIERS = { high: { farRoads: true, caches: true }, low: { farRoads: false, caches: false } } as const;

/**
 * The records' memory budget: the cells nearest the eye first, as many as fit, so the far range ends where it is full.
 * 64 MB holds a disc of 5 km at the fixture's mix of town and country in this layout (64–66 MB; 35–37 MB in the compact
 * far layout), so only a town shrinks it: a disc of town cells fits about 2.3 km.
 */
export const RECORDS_MB = 64;

function flags() {
    try {
        const q = new URLSearchParams(globalThis.location?.search ?? '');
        const tier = TIERS[q.get('tier') === 'low' ? 'low' : 'high'];
        const view = q.get('model') === 'labels' ? 1 : q.get('model') === 'work' ? 2 : q.get('model') === 'work3' ? 3 : q.get('model') === 'work4' ? 4 : q.get('model') === 'reach' ? 5 : 0;
        const farRoads = q.has('farroads') ? q.get('farroads') !== '0' : tier.farRoads;
        const eye = q.has('lat') && q.has('lon') ? { lat: Number(q.get('lat')), lon: Number(q.get('lon')) } : undefined;
        return { model: q.get('ground') === 'model', farRoads, view, probe: Number(q.get('mprobe') ?? 0) | (tier.caches ? 2097152 | 33554432 : 0), eye };
    } catch {
        return { model: false, farRoads: true, view: 0, probe: 0, eye: undefined };
    }
}

/** Whether the page runs the one road model, its tier's settings, its debug view (1 draws the labels flat) and the eye it starts at. */
export const MODEL = flags();

/**
 * The records of the cells nearest `eye` that fit `mb` megabytes, in the same layout: a cell past them has none. Its
 * far radius, the distance of the last cell kept, goes to the console.
 */
export function nearestRecords(words: Uint32Array, eye: { lat: number; lon: number }, mb: number): Uint32Array {
    const [, x0, y0, w, h] = words, head = 5 + w * h;
    const starts = [...new Set(words.subarray(5, head))].filter(o => o).sort((a, b) => a - b);
    const size = new Map(starts.map((o, k) => [o, (starts[k + 1] ?? words.length) - o]));
    const lat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y / 65536))) * 180) / Math.PI;
    const metres = (i: number, j: number) => {
        const dy = (lat(y0 + j + 0.5) - eye.lat) * 111_320, dx = (((x0 + i + 0.5) / 65536) * 360 - 180 - eye.lon) * 111_320 * Math.cos((eye.lat * Math.PI) / 180);
        return Math.hypot(dx, dy);
    };
    const cells = Array.from({ length: w * h }, (_, c) => c).filter(c => words[5 + c]).map(c => ({ c, d: metres(c % w, Math.floor(c / w)) })).sort((a, b) => a.d - b.d);
    const out = new Uint32Array(words.length);
    out.set(words.subarray(0, 5));
    let at = head, radius = 0;
    for (const { c, d } of cells) {
        const n = size.get(words[5 + c])!;
        if ((at + n) * 4 > mb * 1e6) break;
        out.set(words.subarray(words[5 + c], words[5 + c] + n), at);
        out[5 + c] = at;
        at += n;
        radius = d;
    }
    console.info(`model: records ${((at * 4) / 1e6).toFixed(1)} MB, far radius ${Math.round(radius)} m`);
    return out.subarray(0, at);
}

let bytes: Promise<ArrayBuffer | undefined> | undefined;

/** Starts loading the records from beside the feature tiles' TileJSON. */
export function loadModel(featuresUrl: string | undefined): void {
    if (!MODEL.model || !featuresUrl || bytes) return;
    const url = new URL('/trailview-model.bin', featuresUrl);
    bytes = fetch(url).then(r => (r.ok ? r.arrayBuffer() : undefined)).catch(() => undefined);
}

/** The records' words as a texture of MODEL_ROW words per row: one word of zeros without the model. */
export async function modelTexture(device: GPUDevice): Promise<GPUTexture> {
    const body = MODEL.model ? await bytes : undefined;
    const all = body ? new Uint32Array(body) : new Uint32Array(1);
    const words = MODEL.eye && all.length > 1 ? nearestRecords(all, MODEL.eye, RECORDS_MB) : all;
    const rows = Math.max(1, Math.ceil(words.length / MODEL_ROW));
    const padded = new Uint32Array(rows * MODEL_ROW);
    padded.set(words);
    const width = words.length > 1 ? MODEL_ROW : 1;
    const texture = device.createTexture({ label: 'model', size: [width, rows], format: 'r32uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    device.queue.writeTexture({ texture }, padded, { bytesPerRow: width * 4, rowsPerImage: rows }, [width, rows]);
    return texture;
}
