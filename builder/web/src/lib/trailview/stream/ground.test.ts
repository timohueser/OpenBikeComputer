import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PlannerConfig } from '../../planner/config';
import type { Source } from '../../planner/layers/archive';
import { seasonDay } from '../../planner/layers/snow';
import { GroundSource, snowBand } from './ground';

const opens = vi.hoisted(() => new Map<string, () => Promise<unknown>>());
vi.mock('../../planner/layers/archive', async original => ({ ...(await original<object>()), openSource: (url: string) => opens.get(url)!() }));
vi.mock('../../planner/release', () => ({ releaseFetch: async () => ({ ok: true, json: async () => ({}) }) }));
vi.mock('../rules/regions', () => ({ rulesOf: () => ({}) }));

// Workers that build no pages.
class FakeWorker {
    onmessage?: (event: { data: unknown }) => void;
    postMessage({ id }: { id: number }) {
        queueMicrotask(() => this.onmessage!({ data: { id, pages: [], complete: true } }));
    }
}

const DATE = '2026-01-15', SNOW_INDEX = seasonDay(DATE).index;
const source = (get = vi.fn(async () => new Uint8Array(1)), metadata = {}) =>
    ({ metadata, minZoom: 0, maxZoom: 12, bounds: [-180, -85, 180, 85], get }) as unknown as Source;

async function ground(snow?: () => Promise<unknown>) {
    vi.stubGlobal('Worker', FakeWorker);
    opens.clear();
    opens.set('ground', async () => source(undefined, { trailview_format: 3 }));
    const layers: Record<string, string> = { 'trailview-ground': 'ground' };
    if (snow) {
        layers[`trailview-snow-${String(snowBand(SNOW_INDEX)).padStart(2, '0')}`] = 'snow';
        opens.set('snow', snow);
    }
    const opened = (await GroundSource.open({ trailview_rules: 'rules', layers } as unknown as PlannerConfig, DATE))!;
    return (z = 5, x = 3, y = 4) => opened.pages(z, x, y, [0, 1], { weight: 0, snowIndex: SNOW_INDEX, shares: [] }, 0);
}

describe('the snow days of the ground', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('opens a band that failed again, and the pages without it are not complete', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const get = vi.fn(async () => new Uint8Array(512 * 512));
        const snow = vi.fn()
            .mockRejectedValueOnce(new Error('timeout'))
            .mockRejectedValueOnce(new Error('timeout'))
            .mockResolvedValue(source(get, { trailview_format: 3 }));
        const pages = await ground(snow);
        expect((await pages()).complete).toBe(false);
        expect((await pages()).complete).toBe(true);
        expect([snow.mock.calls.length, get.mock.calls.length]).toEqual([3, 1]);
    });

    it('reads the nearest ancestor of a missing snow tile down to zoom 7', async () => {
        const get = vi.fn(async (z: number) => (z === 7 ? new Uint8Array(512 * 512) : undefined));
        const pages = await ground(async () => source(get as unknown as Parameters<typeof source>[0], { trailview_format: 3 }));
        expect((await pages(10, 600, 400)).complete).toBe(true);
        expect(get.mock.calls.map(([z]) => z)).toEqual([10, 9, 8, 7]);
    });

    it('builds complete pages without snow where the release has no band', async () => {
        const pages = await ground();
        expect((await pages()).complete).toBe(true);
    });
});

describe('the decode probe', () => {
    afterEach(() => vi.unstubAllGlobals());

    it('fails the ground once where a worker does not decode the bytes under a low alpha', async () => {
        const terminate = vi.fn();
        vi.stubGlobal('Worker', class {
            onmessage?: (event: { data: unknown }) => void;
            terminate = terminate;
            postMessage({ id }: { id: number }) {
                queueMicrotask(() => this.onmessage!({ data: { id, error: 'premultiplied' } }));
            }
        });
        opens.clear();
        opens.set('ground', async () => source(undefined, { trailview_format: 3 }));
        const config = { trailview_rules: 'rules', layers: { 'trailview-ground': 'ground' } } as unknown as PlannerConfig;
        await expect(GroundSource.open(config, DATE)).rejects.toThrow('premultiplied');
        expect(terminate).toHaveBeenCalledTimes(2);
    });
});
