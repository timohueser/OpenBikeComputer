// Decodes the ground, colour and snow tiles of a material tile and builds its pages, off the main thread.
import { imagePixels } from '../../planner/layers/terrain';
import { groundPages, type GroundDate, type Raster } from './ground-pages';

/** A tile body: an image, or the planar bytes of a snow tile. */
export interface Body { key: string; z: number; x: number; y: number; body: ArrayBuffer; planar?: boolean }

export interface GroundJob {
    z: number; x: number; y: number;
    material?: Body; lithology?: Body; region?: Body; quarters?: [Body | undefined, Body | undefined]; ndvimax?: Body; snow?: (Body | undefined)[];
    seasons: number; date: GroundDate; world: number;
}

/** Decoded tiles by key; a coarse tile serves the pages of many material tiles. */
const decoded = new Map<string, Raster>();
const BUDGET = 96 * 2 ** 20;
let held = 0;

async function raster(body: Body | undefined): Promise<Raster | undefined> {
    if (!body) return undefined;
    const kept = decoded.get(body.key);
    if (kept) {
        decoded.delete(body.key);
        decoded.set(body.key, kept);
        return kept;
    }
    const { z, x, y } = body;
    const out: Raster = body.planar
        ? { z, x, y, size: 256, channels: 1, data: new Uint8Array(body.body) }
        : await imagePixels(new Uint8Array(body.body)).then(image => ({ z, x, y, size: image.width, channels: 4, data: new Uint8Array(image.data.buffer) }));
    decoded.set(body.key, out);
    held += out.data.byteLength;
    for (const [key, old] of decoded) {
        if (held <= BUDGET) break;
        decoded.delete(key);
        held -= old.data.byteLength;
    }
    return out;
}

self.onmessage = async ({ data: { id, job } }: MessageEvent<{ id: number; job: GroundJob }>) => {
    try {
        // A tile of a layer other than the material that does not decode leaves that layer out.
        const [material, lithology, region, a, b, ndvimax, ...snow] = await Promise.all(
            [job.material, job.lithology, job.region, ...(job.quarters ?? [undefined, undefined]), job.ndvimax, ...(job.snow ?? [])]
                .map((body, i) => i === 0 ? raster(body) : raster(body).catch(() => undefined)));
        const pages = groundPages({
            ...job, material, lithology, region, quarters: a || b ? [a, b] : undefined, ndvimax,
            snow: snow.some(Boolean) ? snow : undefined,
        });
        self.postMessage({ id, pages }, { transfer: pages.flatMap(page => (page ? [page.cells.buffer, page.colour.buffer, page.snow.buffer] : [])) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
