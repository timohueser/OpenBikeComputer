import { describe, expect, it } from 'vitest';
import { PAGE, tilePages, UNKNOWN16 } from './heights';

const SIZE = 2 * PAGE;

/** A Terrarium tile whose pixel at (x, y) has the height and alpha of `at`. */
function tile(at: (x: number, y: number) => [height: number, alpha: number]): Uint8Array {
    const rgba = new Uint8Array(4 * SIZE * SIZE);
    for (let y = 0; y < SIZE; y++) for (let x = 0; x < SIZE; x++) {
        const [height, alpha] = at(x, y);
        const v = height + 32768;
        rgba.set([Math.floor(v / 256), Math.floor(v) % 256, Math.round((v % 1) * 256), alpha], 4 * (y * SIZE + x));
    }
    return rgba;
}

describe('the stored pages of a terrain tile', () => {
    it('codes known heights in 16 bits, within half a step', () => {
        const height = (x: number, y: number) => -400 + 37.5 * x + 0.25 * y;
        const [west, east, south, ramp] = tilePages(tile((x, y) => {
            if (x < PAGE && y < PAGE) return [height(x, y), x === 3 ? 254 : 255];
            if (y < PAGE) return [812, 255];
            if (x < PAGE) return [0, 0];
            return [height(x, y), 255];
        }), SIZE);

        // A pixel that is not opaque is unknown.
        expect(west!.complete).toBe(false);
        expect(west!.texels[3]).toBe(UNKNOWN16);
        // A page with one height keeps a scale above 0.
        expect(east).toMatchObject({ min: 812, max: 812, complete: true, offset: 812, scale: 1e-4 });
        expect(south).toBeNull();

        let top = 0;
        for (let j = 0; j < PAGE; j++) for (let i = 0; i < PAGE; i++) {
            const code = ramp!.texels[j * PAGE + i];
            top = Math.max(top, code);
            expect(Math.abs(ramp!.offset + code * ramp!.scale - height(i + PAGE, j + PAGE))).toBeLessThanOrEqual(ramp!.scale / 2 + 1e-9);
        }
        // The greatest height takes the last code below the unknown one.
        expect(top).toBe(UNKNOWN16 - 1);
    });
});
