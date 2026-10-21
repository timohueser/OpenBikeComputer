import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PlannerConfig } from '../../planner/config';
import { GroundSource } from './ground';
import type { GroundJob } from './ground-worker';

const TILES = 'https://tiles.test';
const rules = { format: 1, materials: { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] }, regions: [{ id: 'world', leaf: 'mixed', look: { snap: 0.4 } }] };
const tileJson = (name: string) => ({
    tiles: [`${TILES}/${name}/{z}/{x}/{y}`], minzoom: 0, maxzoom: name === 'snow' ? 12 : 13,
    ...(name === 'snow' ? { first_season: 2015, seasons: 10, step_days: 2 } : {}),
});

/** A worker that answers each job with the job itself, so the test sees what the pages would be built from. */
class EchoWorker {
    onmessage?: (event: { data: unknown }) => void;
    postMessage({ id, job }: { id: number; job: GroundJob }) {
        setTimeout(() => this.onmessage?.({ data: { id, pages: [job, null, null, null] } }));
    }
}

describe('the ground of a release', () => {
    afterEach(() => vi.unstubAllGlobals());

    it('builds the pages of a material tile without a layer whose tile fails, and fails them without the material', async () => {
        vi.stubGlobal('Worker', EchoWorker);
        vi.stubGlobal('fetch', async (url: string) => {
            const path = new URL(url).pathname;
            if (path === '/trailview/rules.json') return Response.json(rules);
            if (path.endsWith('.json')) return Response.json(tileJson(path.slice(1, -5)));
            if (path.startsWith('/snow/') || path === '/trailview-material/13/4290/2886') return new Response(null, { status: 500 });
            return new Response(new Uint8Array([1, 2, 3]));
        });
        const config = {
            trailview_rules: `${TILES}/trailview/rules.json`,
            layers: { 'trailview-material': `${TILES}/trailview-material.json`, 'trailview-region': `${TILES}/trailview-region.json`, snow: `${TILES}/snow.json` },
        } as unknown as PlannerConfig;
        const source = (await GroundSource.open(config))!;
        const date = { weight: 0, snowIndex: 0, shares: [0.5] };
        const [job] = (await source.pages(13, 4290, 2885, [0, 1], date, 0)) as unknown as GroundJob[];
        expect(job.material?.key).toBe('material/13/4290/2885');
        expect(job.region?.key).toBe('region/13/4290/2885');
        expect(job.snow?.every(tile => tile === undefined)).toBe(true);
        await expect(source.pages(13, 4290, 2886, [0, 1], date, 0)).rejects.toThrow('500');
    });
});
