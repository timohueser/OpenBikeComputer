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
    return () => opened.pages(5, 3, 4, [0, 1], { weight: 0, snowIndex: SNOW_INDEX, shares: [] }, 0);
}

describe('the snow days of the ground', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('opens a band that failed again, and the pages without it are not complete', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const get = vi.fn(async () => new Uint8Array(512 * 512));
        const snow = vi.fn()
            .mockRejectedValueOnce(new Error('timeout'))
            .mockRejectedValueOnce(new Error('timeout'))
            .mockResolvedValue(source(get));
        const pages = await ground(snow);
        expect((await pages()).complete).toBe(false);
        expect((await pages()).complete).toBe(true);
        expect([snow.mock.calls.length, get.mock.calls.length]).toEqual([3, 1]);
    });

    it('builds complete pages without snow where the release has no band', async () => {
        const pages = await ground();
        expect((await pages()).complete).toBe(true);
    });
});
