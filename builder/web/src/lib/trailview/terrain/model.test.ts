import { describe, expect, it } from 'vitest';
import { nearestRecords } from './model';

describe('nearestRecords', () => {
    it('keeps the cells nearest the eye that fit the budget, each block whole and relocated', () => {
        // Three cells in a row of zoom 16 at about 46.8° N, blocks of 10, 20 and 30 words: the eye over the last one.
        const [x0, y0] = [34300, 23100];
        const words = new Uint32Array(8 + 60);
        words.set([0x4f424d46, x0, y0, 3, 1, 8, 18, 38]);
        for (let k = 8; k < words.length; k++) words[k] = k;
        const lon = ((x0 + 2.5) / 65536) * 360 - 180;
        const lat = (Math.atan(Math.sinh(Math.PI * (1 - (2 * (y0 + 0.5)) / 65536))) * 180) / Math.PI;
        const kept = nearestRecords(words, { lat, lon }, ((8 + 30 + 20) * 4) / 1e6);
        expect([...kept.subarray(5, 8)]).toEqual([0, 38, 8]);
        expect([...kept.subarray(8, 38)]).toEqual([...words.subarray(38, 68)]);
        expect([...kept.subarray(38, 58)]).toEqual([...words.subarray(18, 38)]);
        expect(kept.length).toBe(58);
    });
});
