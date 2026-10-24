// Decodes the ground, colour and snow tiles of a ground tile and builds its pages, off the main thread.
import { groundPages, type GroundDate, type Raster } from './ground-pages';

/** A tile body: an image, or the bytes of a snow day. */
export interface Body { key: string; z: number; x: number; y: number; body: ArrayBuffer; planar?: boolean }

export interface GroundJob {
    z: number; x: number; y: number;
    ground?: Body; quarters?: [Body | undefined, Body | undefined]; snow?: Body;
    date: GroundDate; world: number;
}

/** Decoded images by key, and the bytes of each that has arrived; a coarse tile serves the pages of many ground tiles. */
const decoded = new Map<string, Promise<Raster>>();
const sizes = new Map<string, number>();
const BUDGET = 96 * 2 ** 20;
let held = 0;

let gl: WebGL2RenderingContext | null | undefined;

/**
 * The RGBA bytes of an image as stored. A 2D canvas premultiplies alpha, which loses the bytes under
 * an alpha below 255, as the region of a ground tile, so the pixels go through a WebGL texture.
 */
async function straightPixels(body: ArrayBuffer): Promise<{ size: number; data: Uint8Array }> {
    if (!gl || gl.isContextLost()) gl = new OffscreenCanvas(1, 1).getContext('webgl2');
    if (!gl) throw new Error('The ground decodes through WebGL 2, which this worker does not have.');
    const bitmap = await createImageBitmap(new Blob([body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    const texture = gl.createTexture(), framebuffer = gl.createFramebuffer();
    try {
        gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
        gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
        gl.bindTexture(gl.TEXTURE_2D, texture);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, bitmap);
        gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
        const data = new Uint8Array(4 * bitmap.width * bitmap.height);
        gl.readPixels(0, 0, bitmap.width, bitmap.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        if (gl.isContextLost()) throw new Error('The WebGL context of the decode was lost.');
        return { size: bitmap.width, data };
    } finally {
        gl.deleteFramebuffer(framebuffer);
        gl.deleteTexture(texture);
        bitmap.close();
    }
}

function raster(body: Body | undefined): Promise<Raster | undefined> {
    if (!body) return Promise.resolve(undefined);
    const { key, z, x, y } = body;
    // A planar body is its raster, and the main thread keeps it.
    if (body.planar) return Promise.resolve({ z, x, y, size: Math.sqrt(body.body.byteLength), channels: 1, data: new Uint8Array(body.body) });
    let entry = decoded.get(key);
    if (entry) {
        decoded.delete(key);
        decoded.set(key, entry);
        return entry;
    }
    entry = straightPixels(body.body).then(({ size, data }) => ({ z, x, y, size, channels: 4, data }));
    decoded.set(key, entry);
    entry.then(out => {
        if (decoded.get(key) !== entry) return;
        sizes.set(key, out.data.byteLength);
        held += out.data.byteLength;
        // A decode in flight has no size yet and stays.
        for (const old of decoded.keys()) {
            if (held <= BUDGET) break;
            const bytes = sizes.get(old);
            if (bytes === undefined) continue;
            decoded.delete(old);
            sizes.delete(old);
            held -= bytes;
        }
    }, () => decoded.delete(key));
    return entry;
}

self.onmessage = async ({ data: { id, job } }: MessageEvent<{ id: number; job: GroundJob }>) => {
    try {
        // A tile of a layer other than the ground that does not decode leaves that layer out, and the pages are not complete.
        let complete = true;
        const [ground, a, b, snow] = await Promise.all(
            [job.ground, ...(job.quarters ?? [undefined, undefined]), job.snow]
                .map((body, i) => i === 0 ? raster(body) : raster(body).catch(() => void (complete = false))));
        const pages = groundPages({ ...job, ground, quarters: a || b ? [a, b] : undefined, snow });
        self.postMessage({ id, pages, complete }, { transfer: pages.flatMap(page => (page ? [page.cells.buffer, page.colour.buffer, page.snow.buffer] : [])) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
