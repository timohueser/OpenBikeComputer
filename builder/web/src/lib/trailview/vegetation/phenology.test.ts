import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { phaseOf, phasesOn, phaseShares, REFERENCE_M, TREELINE_M, TURNS } from './phenology';

const shader = readFileSync(new URL('./phenology.wgsl', import.meta.url), 'utf8');
const hashes = Array.from({ length: 200 }, (_, k) => (k + 0.5) / 200);

describe("the trees' year", () => {
    it('has the turns of phenology.wgsl', () => {
        const turns = [...shader.matchAll(/^const TURN_\w+: vec3f = vec3f\(([^)]+)\);/gm)].map(([, values]) => values.split(',').map(Number));
        expect(turns).toEqual(TURNS.map(turn => [...turn]));
        expect(shader).toContain(`const REFERENCE_M: f32 = ${REFERENCE_M.toFixed(1)};`);
        expect(shader).toContain(`const TREELINE_M: f32 = ${TREELINE_M.toFixed(1)};`);
    });

    it('takes each tree through winter, spring, summer and autumn once a year, never back', () => {
        for (const elevation of [0, 500, 1500, TREELINE_M]) for (const u of hashes) {
            let phase = phaseOf(u, 0, elevation), turns = 0;
            for (let day = 1; day < 365; day++) {
                const next = phaseOf(u, day, elevation);
                if (next !== phase) {
                    expect(next, `u ${u} at ${elevation} m on day ${day}`).toBe((phase + 1) % 4);
                    turns++;
                }
                phase = next;
            }
            expect(turns).toBe(4);
        }
    });

    it('puts the share of the hashes in each phase that phaseShares gives', () => {
        for (const day of [100, 120, 160, 260, 290, 310]) for (const elevation of [400, 1800]) {
            const counts = [0, 0, 0, 0];
            for (const u of hashes) counts[phaseOf(u, day, elevation)]++;
            phaseShares(day, elevation).forEach((share, phase) => expect(counts[phase] / hashes.length).toBeCloseTo(share, 1));
        }
    });

    it('turns higher trees later in spring and earlier in autumn', () => {
        expect(phaseShares(120, 1500)[0]).toBeGreaterThan(phaseShares(120, 500)[0]);
        expect(phaseShares(270, 1500)[3]).toBeGreaterThan(phaseShares(270, 500)[3]);
    });

    it('builds every phase that a tree has on a day', () => {
        const missing: string[] = [];
        for (let day = 0; day < 365; day++) {
            const phases = phasesOn(day);
            for (let elevation = 0; elevation <= TREELINE_M + 500; elevation += 37) for (const u of hashes) {
                if (!phases.has(phaseOf(u, day, elevation))) missing.push(`day ${day} at ${elevation} m`);
            }
        }
        expect(missing).toEqual([]);
    });
});
