// The pages of a stored terrain tile: a 512 px Terrarium tile holds the 256 × 256 texel pages of its
// four nodes one level finer. Workers use this module.

/** Texels along a page edge. */
export const PAGE = 256;
/** The raw texel of a height that is not known: a float32 NaN for `r32float`, the top code for `r16`. */
export const UNKNOWN32 = 0xffffffff;
export const UNKNOWN16 = 0xffff;

export type PageFormat = 'r32float' | 'r16';

/**
 * One page: its texels as stored on the GPU, the range of its known heights, and whether it knows
 * every height. `r16` stores offset + code × scale.
 */
export interface PageData { texels: Uint32Array<ArrayBuffer> | Uint16Array<ArrayBuffer>; min: number; max: number; complete: boolean; offset: number; scale: number }

/**
 * The pages of the north-west, north-east, south-west and south-east quadrants of a tile's RGBA
 * pixels; null for a quadrant without a known height. A pixel is R × 256 + G + B / 256 − 32768
 * metres, and unknown unless it is opaque.
 */
export function tilePages(rgba: ArrayLike<number>, size: number, format: PageFormat): (PageData | null)[] {
    if (size !== 2 * PAGE) throw new Error(`A terrain tile has ${size} px, not ${2 * PAGE}.`);
    return [0, 1, 2, 3].map(q => {
        const heights = new Float32Array(PAGE * PAGE);
        let min = Infinity, max = -Infinity, known = 0;
        for (let j = 0; j < PAGE; j++) for (let i = 0; i < PAGE; i++) {
            const p = 4 * ((j + (q >> 1) * PAGE) * size + i + (q & 1) * PAGE);
            const h = rgba[p + 3] === 255 ? rgba[p] * 256 + rgba[p + 1] + rgba[p + 2] / 256 - 32768 : NaN;
            heights[j * PAGE + i] = h;
            if (h < min) min = h;
            if (h > max) max = h;
            if (!Number.isNaN(h)) known++;
        }
        return known ? pageData(heights, min, max, known === PAGE * PAGE, format) : null;
    });
}

/** Heights (NaN where unknown) as the texels of a page format. */
export function pageData(heights: Float32Array<ArrayBuffer>, min: number, max: number, complete: boolean, format: PageFormat): PageData {
    if (format === 'r32float') {
        const texels = new Uint32Array(heights.buffer.slice(0));
        heights.forEach((h, i) => { if (Number.isNaN(h)) texels[i] = UNKNOWN32; });
        return { texels, min, max, complete, offset: 0, scale: 1 };
    }
    const scale = Math.max((max - min) / (UNKNOWN16 - 1), 1e-4);
    return { texels: Uint16Array.from(heights, h => (Number.isNaN(h) ? UNKNOWN16 : Math.round((h - min) / scale))), min, max, complete, offset: min, scale };
}
