import { describe, expect, it } from 'vitest';
import { packPoints, POINT_WORDS } from './points';

describe('the point instances', () => {
    it('pack a point as points.wgsl reads it: world units, model, turn and tint in one word, the height as f16', () => {
        const words = packPoints([{ x: 2 ** 29 + 5, y: 123456, yaw: Math.PI, height: 12.5, model: 7, tint: 1.25 }]);
        expect(words.length).toBe(POINT_WORDS);
        expect([...new Int32Array(words.buffer, 0, 2)]).toEqual([2 ** 29 + 5, 123456]);
        expect(words[2] & 0xffff).toBe(7);
        expect((words[2] >> 16) & 255).toBe(128);
        expect(words[2] >>> 24).toBe(255);
        // 12.5 = 1.5625 × 2^3: exponent 18, mantissa 0x240.
        expect(words[3] & 0xffff).toBe((18 << 10) | 0x240);
    });
});
