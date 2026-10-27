import { describe, expect, it } from 'vitest';
import { SEASONS } from '../rules/regions';
import { plant, PLANTS } from './plant-library';
import { POLE_SEED } from './plants/agave';

/** The seeds that vegetation/plants.ts builds a plant in (SEEDS). */
const SEEDS = [1, 2, 3];

describe('the plant models', () => {
    it('build every plant in every season and shape: 1 tall, a sized one in its tallest season, with finite vertices', () => {
        for (const [id, { sized }] of Object.entries(PLANTS)) for (const seed of SEEDS) {
            const tops = SEASONS.map(season => {
                const { position, normal, color, index } = plant(id, season, seed);
                expect([...position, ...normal, ...color].every(Number.isFinite), `${id} ${season} ${seed}`).toBe(true);
                expect(color.length, `${id} ${season} ${seed}`).toBe(position.length);
                expect(Math.max(...index)).toBeLessThan(position.length / 3);
                const top = Math.max(...position.filter((_, i) => i % 3 === 2));
                if (!sized) expect(top, `${id} ${season} ${seed}`).toBeCloseTo(1, 6);
                return top;
            });
            // An agave's flower pole stands far above the rosette that its height measures.
            if (sized && !(id === 'agave' && seed === POLE_SEED)) expect(Math.abs(Math.max(...tops) - 1), `${id} ${seed}`).toBeLessThan(0.1);
        }
    });
});
