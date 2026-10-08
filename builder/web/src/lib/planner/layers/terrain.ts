// Terrain heights from Terrarium tiles, such as the map's terrain source. Workers use this module too.
import { decodedTiles, type TileGetter, type Tiles } from './archive';

/** A height that is not known: a pixel that is not opaque. R = G = 255 also decodes to it. */
export const UNKNOWN_HEIGHT = 32767;
/** A 512 px tile of whole metres takes 512 KiB. */
const BUDGET = 16 * 2 ** 20;

/** The RGBA pixels of an image tile as stored: colour conversion or premultiplied alpha would change the heights. */
export async function imagePixels(body: Uint8Array<ArrayBuffer>): Promise<ImageData> {
    const bitmap = await createImageBitmap(new Blob([body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    try {
        const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext('2d', { willReadFrequently: true })!;
        context.drawImage(bitmap, 0, 0);
        return context.getImageData(0, 0, bitmap.width, bitmap.height);
    } finally {
        bitmap.close();
    }
}

/** Whole metres of the first `count` pixels: R × 256 + G − 32768. */
export function heights(rgba: ArrayLike<number>, count: number): Int16Array {
    const out = new Int16Array(count);
    for (let i = 0; i < count; i++) out[i] = rgba[4 * i + 3] === 255 ? rgba[4 * i] * 256 + rgba[4 * i + 1] - 32768 : UNKNOWN_HEIGHT;
    return out;
}

/** The heights of each Terrarium tile, decoded once into a bounded cache. */
export function terrainHeights(get: TileGetter): Tiles<Int16Array> {
    return decodedTiles(get, async body => {
        const rgba = await imagePixels(body);
        return heights(rgba.data, rgba.width * rgba.height);
    }, BUDGET, tile => tile.byteLength);
}
