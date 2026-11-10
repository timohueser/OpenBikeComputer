import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PlannerConfig } from '../../planner/config';
import type { Source } from '../../planner/layers/archive';
import { GroundSource } from './ground';

const opens = vi.hoisted(() => new Map<string, () => Promise<unknown>>());
vi.mock('../../planner/layers/archive', async original => ({ ...(await original<object>()), openSource: (url: string) => opens.get(url)!() }));
vi.mock('../../planner/release', () => ({ releaseFetch: async () => ({ ok: true, json: async () => ({}) }) }));
vi.mock('../rules/regions', () => ({ rulesOf: () => ({ regions: [] }) }));

// Workers that build no pages.
class FakeWorker {
    onmessage?: (event: { data: unknown }) => void;
    postMessage({ id }: { id: number }) {
        queueMicrotask(() => this.onmessage!({ data: { id, pages: [], complete: true } }));
    }
}

const source = (get = vi.fn(async () => new Uint8Array(1)), metadata = {}) =>
    ({ metadata, minZoom: 0, maxZoom: 12, bounds: [-180, -85, 180, 85], get }) as unknown as Source;

async function ground(snow?: () => Promise<unknown>) {
    vi.stubGlobal('Worker', FakeWorker);
    opens.clear();
    opens.set('ground', async () => source(undefined, { trailview_format: 3 }));
    const layers: Record<string, string> = { 'trailview-ground': 'ground' };
    if (snow) {
        layers.snow = 'snow';
        opens.set('snow', snow);
    }
    const opened = (await GroundSource.open({ trailview_rules: 'rules', layers } as unknown as PlannerConfig))!;
    return (z = 5, x = 3, y = 4, band = 10) => opened.pages(z, x, y, [0, 1], { weight: 0, band }, 0);
}

describe('the snow of the ground', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('reads the snow tile of a ground tile once for dates in different bands', async () => {
        const get = vi.fn(async () => new Uint8Array(512 * 512));
        const pages = await ground(async () => source(get, { snow_format: 1 }));
        expect((await pages(5, 3, 4, 22)).complete).toBe(true);
        expect((await pages(5, 3, 4, 38)).complete).toBe(true);
        expect(get).toHaveBeenCalledTimes(1);
    });

    it('opens a snow archive that failed again, and the pages without it are not complete', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const get = vi.fn(async () => new Uint8Array(512 * 512));
        const snow = vi.fn()
            .mockRejectedValueOnce(new Error('timeout'))
            .mockRejectedValueOnce(new Error('timeout'))
            .mockResolvedValue(source(get, { snow_format: 1 }));
        const pages = await ground(snow);
        expect((await pages()).complete).toBe(false);
        expect((await pages()).complete).toBe(true);
        expect([snow.mock.calls.length, get.mock.calls.length]).toEqual([3, 1]);
    });

    it('leaves out a snow archive of another format, and the pages are complete', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const get = vi.fn(async () => new Uint8Array(512 * 512));
        const snow = vi.fn(async () => source(get, { snow_format: 2 }));
        const pages = await ground(snow);
        expect((await pages()).complete).toBe(true);
        expect((await pages()).complete).toBe(true);
        expect([snow.mock.calls.length, get.mock.calls.length, warn.mock.calls.length]).toEqual([1, 0, 1]);
    });

    it('reads the nearest ancestor of a missing snow tile down to zoom 7', async () => {
        const get = vi.fn(async (z: number) => (z === 7 ? new Uint8Array(512 * 512) : undefined));
        const pages = await ground(async () => source(get as unknown as Parameters<typeof source>[0], { snow_format: 1 }));
        expect((await pages(10, 600, 400)).complete).toBe(true);
        expect(get.mock.calls.map(([z]) => z)).toEqual([10, 9, 8, 7]);
    });

    it('builds complete pages without snow where the release has no snow archive', async () => {
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
        await expect(GroundSource.open(config)).rejects.toThrow('premultiplied');
        expect(terminate).toHaveBeenCalledTimes(2);
    });
});
