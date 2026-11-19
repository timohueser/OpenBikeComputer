// The RGBA bytes of WebP tiles as stored: the browser's decode where it gives them, else the WASM decoder. Workers use this module.
import { imagePixels } from '../../planner/layers/terrain';
import initWebp, { decode_webp } from './webp/obc_webp_bridge.js';

export type Pixels = { size: number; data: Uint8Array };

let loaded: Promise<unknown> | undefined;

/** The bytes of a WebP image in WASM: slower than the browser's decode, and exact where that is not (WebKit). */
export async function wasmPixels(body: ArrayBuffer): Promise<Pixels> {
    loaded ??= initWebp().catch(error => { throw new Error(`The WASM decode of the tiles does not load: ${error}`); });
    await loaded;
    const size = new Uint32Array(1);
    const data = decode_webp(new Uint8Array(body), size);
    return { size: size[0], data };
}

/** Whether every alpha is 0 or 255: a terrain tile stores no other (specs/planner-release.md, Terrain tiles). */
export function terrainAlpha(rgba: ArrayLike<number>): boolean {
    for (let i = 3; i < rgba.length; i += 4) if (rgba[i] !== 0 && rgba[i] !== 255) return false;
    return true;
}

let browserFails = false;

/**
 * The pixels of a terrain tile. The browser decodes it; a tile with an alpha other than 0 or 255 is
 * decoded again in WASM (WebKit can hand back changed pixels after a cold start), and so is every
 * later tile of this worker.
 */
export async function terrainPixels(body: ArrayBuffer): Promise<Pixels> {
    if (!browserFails) {
        const { data, width } = await imagePixels(new Uint8Array(body));
        if (terrainAlpha(data)) return { size: width, data: new Uint8Array(data.buffer, data.byteOffset, data.byteLength) };
        browserFails = true;
        console.info('TrailView decodes the terrain tiles in WASM: the browser gave a tile an alpha other than 0 or 255.');
    }
    return wasmPixels(body);
}
