import { describe, expect, it } from 'vitest';
import { SEASONS } from '../rules/regions';
import { plant, PLANTS } from './plant-library';

describe('the plant models', () => {
    it('build every plant in every season: 1 tall, with finite vertices', () => {
        for (const id of Object.keys(PLANTS)) for (const season of SEASONS) {
            const { position, normal, color, index } = plant(id, season, 2);
            expect([...position, ...normal, ...color].every(Number.isFinite), `${id} ${season}`).toBe(true);
            expect(color.length, `${id} ${season}`).toBe(position.length);
            expect(Math.max(...position.filter((_, i) => i % 3 === 2))).toBeCloseTo(1, 6);
            expect(Math.max(...index)).toBeLessThan(position.length / 3);
        }
    });
});
