// Decodes the ground, colour and snow tiles of a ground tile and builds its pages, off the main thread.
import { colourTexels, groundPages, type GroundDate, type Raster } from './ground-pages';

/** A tile body of a layer of specs/trailview-tiles.md. */
export interface Body { key: string; z: number; x: number; y: number; body: ArrayBuffer; layer: 'ground' | 'colour' | 'snow' }

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

/**
 * A 2 × 2 lossless WebP of (200, 100, 50) under alpha 0, 1, 2 and 255. Every ground texel has an alpha
 * of at most 61, so a decode that premultiplies scales every byte of the ground.
 */
const PROBE = 'UklGRioAAABXRUJQVlA4TB4AAAAvAUAAEB8wZIM8MgKSJP9vDuIBYJANUKYKs4j+xw4=';
let probed: Promise<void> | undefined;

/** Fails where this worker does not decode the RGB under a low alpha exactly. */
function probe(): Promise<void> {
    probed ??= straightPixels(Uint8Array.from(atob(PROBE), c => c.charCodeAt(0)).buffer).then(({ data }) => {
        const exact = [0, 1, 2, 255].every((alpha, i) => [200, 100, 50, alpha].every((byte, c) => data[4 * i + c] === byte));
        if (!exact) throw new Error('This browser does not decode the RGB under a low alpha exactly, which the ground needs.');
    });
    return probed;
}

/** The bytes of a tile as `groundPages` reads them. */
async function decode({ body, layer }: Body): Promise<{ size: number; channels: number; data: Uint8Array }> {
    if (layer === 'colour') {
        // The NDVI tile, then the colour: the RIFF header of the NDVI tile gives its length.
        const split = 8 + new DataView(body).getUint32(4, true);
        const [ndvi, rgb] = await Promise.all([straightPixels(body.slice(0, split)), straightPixels(body.slice(split))]);
        if (ndvi.size !== 256 || rgb.size !== 512) throw new Error(`A colour tile has a ${ndvi.size} px NDVI tile and ${rgb.size} px colour.`);
        return { size: rgb.size, channels: 4, data: colourTexels(ndvi.data, rgb.data) };
    }
    const { size, data } = await straightPixels(body);
    if (layer === 'ground') return { size, channels: 4, data };
    // A snow day is gray: its byte is the red of each pixel.
    const gray = new Uint8Array(size * size);
    for (let i = 0; i < gray.length; i++) gray[i] = data[4 * i];
    return { size, channels: 1, data: gray };
}

function raster(body: Body | undefined): Promise<Raster | undefined> {
    if (!body) return Promise.resolve(undefined);
    const { key, z, x, y } = body;
    let entry = decoded.get(key);
    if (entry) {
        decoded.delete(key);
        decoded.set(key, entry);
        return entry;
    }
    entry = decode(body).then(pixels => ({ z, x, y, ...pixels }));
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

/** A message without a job asks for the probe. */
self.onmessage = async ({ data: { id, job } }: MessageEvent<{ id: number; job?: GroundJob }>) => {
    try {
        if (!job) {
            await probe();
            return void self.postMessage({ id });
        }
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
