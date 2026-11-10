import { describe, expect, it } from 'vitest';
import { PAGE } from '../stream/heights';
import { cliffJobs } from './cliff';
import { key, type Node } from './lod';
import type { Page } from './pages';

const page = (x: number, y: number, mask?: number): Page => ({ node: { level: 17, x, y }, kind: 'synth', slot: x + 10 * y, used: 0, range: [0, 0], complete: true, ready: true, drawn: x + 10 * y, mask });
const pool = (...list: Page[]) => new Map(list.map(p => [key(p.node), p]));
const rects = (jobs: ReturnType<typeof cliffJobs>) => jobs.map(({ page: { node }, lo, hi }) => `${node.x},${node.y}: ${lo} ${hi}`).sort();

describe('cliffJobs', () => {
    it('makes a page whole when its mask or heights change, and the texels of each neighbour with a mask that face it', () => {
        const last = new Map<string, [number, Node]>();
        const west = page(4, 5, 0), middle = page(5, 5, 1), north = page(5, 4, 2), corner = page(6, 6, 3), plain = page(6, 5);
        expect(cliffJobs(pool(west, north, corner, plain), last, [])).toHaveLength(3);
        // The middle page gets its mask: its neighbours make the 4 texels that read it.
        expect(rects(cliffJobs(pool(west, middle, north, corner, plain), last, []))).toEqual([
            `4,5: ${PAGE - 4},0 ${PAGE},${PAGE}`, `5,4: 0,${PAGE - 4} ${PAGE},${PAGE}`, `5,5: 0,0 ${PAGE},${PAGE}`, '6,6: 0,0 4,4',
        ]);
        expect(cliffJobs(pool(west, middle, north, corner, plain), last, [])).toEqual([]);
        // New heights of the page without a mask, and the north page loses its mask.
        north.mask = undefined;
        expect(rects(cliffJobs(pool(west, middle, north, corner, plain), last, [plain.node]))).toEqual([
            `4,5: ${PAGE - 4},0 ${PAGE},4`, `5,5: 0,0 ${PAGE},4`, `5,5: ${PAGE - 4},0 ${PAGE},${PAGE}`, `6,6: 0,0 ${PAGE},4`,
        ]);
    });
});
