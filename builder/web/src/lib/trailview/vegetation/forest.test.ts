import { describe, expect, it } from 'vitest';
import { densest } from './canopy';
import { bluePoints, type Species, speciesTable } from './forest';

/** The least distance between the first n points, across the edges of the unit square. */
function spacing(points: Float32Array, n: number): number {
    let least = Infinity;
    for (let i = 0; i < n; i++) for (let j = 0; j < i; j++) {
        const dx = Math.abs(points[2 * i] - points[2 * j]), dy = Math.abs(points[2 * i + 1] - points[2 * j + 1]);
        least = Math.min(least, Math.hypot(Math.min(dx, 1 - dx), Math.min(dy, 1 - dy)));
    }
    return least;
}

/** species_of of forest.wgsl over the table, in f32. */
function lookup(table: Uint32Array, region: number, elevation: number, leaf: number, pick: number): number {
    const f = new Float32Array(table.buffer), at = table[1 + region], n = table[at];
    let below = 0;
    while (below < n && f[at + 1 + below] < elevation) below++;
    const list = at + 1 + n + 3 * (3 * (2 * below + (below < n && f[at + 1 + below] === elevation ? 1 : 0)) + leaf);
    let left = Math.fround(pick * f[list + 2]), model = -1;
    for (let k = 0; k < table[list + 1]; k++) {
        model = table[table[list] + 2 * k];
        left = Math.fround(left - f[table[list] + 2 * k + 1]);
        if (left < 0) break;
    }
    return model;
}

/** The scan of every species that the table stands for, up to the first without weight: the first rule with any weight, then by weight. */
function scan(all: Species[], elevation: number, leaf: number, pick: number): number {
    const ended = all.findIndex(s => !(Math.fround(s.weight) > 0)), species = ended < 0 ? all : all.slice(0, ended);
    const rules = species.map(s => {
        const band = elevation >= Math.fround(s.band[0]) && elevation <= Math.fround(s.band[1]), same = leaf === 0 || s.leaf === leaf;
        return [band && same, band, same, true];
    });
    const totals = [0, 1, 2, 3].map(r => species.reduce((sum, s, k) => (rules[k][r] ? Math.fround(sum + Math.fround(s.weight)) : sum), 0));
    const rule = [0, 1, 2].find(r => totals[r] > 0) ?? 3;
    let left = Math.fround(pick * totals[rule]), model = -1;
    for (const [k, s] of species.entries()) {
        if (!rules[k][rule]) continue;
        model = s.model;
        left = Math.fround(left - Math.fround(s.weight));
        if (left < 0) break;
    }
    return model;
}

describe('the forest', () => {
    it('picks from its table the species that a scan of every species picks', () => {
        const regions: Species[][] = [
            [{ model: 3, leaf: 2, band: [800, 1900], weight: 4 }, { model: 5, leaf: 1, band: [400, 1200], weight: 2.5 }, { model: 7, leaf: 2, band: [1500, 2300], weight: 1 }, { model: 9, leaf: 1, band: [0, 900], weight: 0.3 }],
            [{ model: 11, leaf: 1, band: [1000, 1000], weight: 1 }],
            [],
            // A weight of 0 ends the list.
            [{ model: 13, leaf: 1, band: [0, 1500], weight: 1 }, { model: 15, leaf: 2, band: [1000, 3000], weight: 0 }, { model: 17, leaf: 2, band: [0, 3000], weight: 2 }],
            // A full pack: 32 species of every band and leaf type.
            Array.from({ length: 32 }, (_, k) => ({ model: 20 + k, leaf: 1 + (k % 2), band: [(k * 137) % 2000, (k * 137) % 2000 + 300 + ((k * 53) % 900)] as [number, number], weight: 0.25 + ((k * 7) % 5) })),
        ];
        const table = speciesTable(regions);
        for (const [r, species] of regions.entries())
            for (const elevation of [-1e30, 0, 350, 899.9, 1150, 2299.5, 4000, ...species.flatMap(({ band }) => band.flatMap(e => [e - 0.5, e, e + 0.5]))])
                for (const leaf of [0, 1, 2])
                    for (const pick of [0, 0.2, 0.5, 0.71, 0.999])
                        expect(lookup(table, r, Math.fround(elevation), leaf, pick)).toBe(scan(species, Math.fround(elevation), leaf, pick));
    });

    it('places progressive blue noise: every first n points keep apart as an even spread of n would', () => {
        const points = bluePoints(512);
        expect(points.every(v => v >= 0 && v < 1)).toBe(true);
        // An even grid of n points is 1 / √n apart; white noise of 512 points comes far closer.
        for (const n of [8, 32, 128, 512]) expect(spacing(points, n) * Math.sqrt(n)).toBeGreaterThan(0.45);
    });

    it('bounds the density of a canopy cell by the levels above it', () => {
        const codes = new Uint16Array(256 * 256);
        // Height 20 m, cover 90 %, stems class 3; height 6 m, cover 40 %, stems class 6; open land; unknown.
        codes.set([40 | (9 << 7) | (3 << 11) | (2 << 14), 12 | (4 << 7) | (6 << 11), 0, 0xffff]);
        const levels = densest(codes);
        expect(levels.map(level => level.length)).toEqual([256, 128, 64, 32, 16, 8, 4, 2, 1].map(side => side * side));
        expect(levels[1][0]).toBe(12 | (9 << 7) | (6 << 11));
        expect(levels[8][0]).toBe(12 | (9 << 7) | (6 << 11));
        expect(levels[1][1]).toBe(0);
    });
});
