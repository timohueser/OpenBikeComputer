import { describe, expect, it } from 'vitest';
import { SEASONS } from '../rules/regions';
import { tree, TREES } from './library';

describe('the tree models', () => {
    it('build every species in every season at every level: 1 tall, with finite vertices, fewer triangles a level', () => {
        for (const id of Object.keys(TREES)) for (const season of SEASONS) {
            const triangles = ([0, 1, 2] as const).map(lod => {
                const { position, normal, color, index } = tree(id, season, lod);
                expect([...position, ...normal, ...color].every(Number.isFinite), `${id} ${season} ${lod}`).toBe(true);
                expect(Math.max(...position.filter((_, i) => i % 3 === 2))).toBeCloseTo(1, 6);
                expect(Math.max(...index)).toBeLessThan(position.length / 3);
                return index.length / 3;
            });
            expect(triangles[2], `${id} ${season}`).toBeLessThan(triangles[1]);
        }
    }, 30_000);
});
