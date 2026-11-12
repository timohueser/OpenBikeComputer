import { IDBFactory } from 'fake-indexeddb';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CoverRecord, RouteRecord } from '../library/records';
import { Library } from '../library/store';
import type { Still, StillJob } from '../trailview/still';
import { lineKey, settled, startCoverJob, wantCover, type Renderer } from './job';
import type { MapPictureMaker, MapPictureResult } from './map-picture';

const route = (id: string, name = id, line = [8.38, 46.82, 8.42, 46.81]): RouteRecord => ({
    id, revision: 0, name, updatedAt: Date.UTC(2026, 7, 15), trip: {},
    summary: { line, heights: [null, null], bounds: [8.38, 46.81, 8.42, 46.82], lengthM: 3000, climbM: 0, highestM: null, seconds: 600, bike: 'road', days: 1 },
});
const result: Still = {
    image: new Blob(['cover']),
    points: { x: Float32Array.of(10, 20), y: Float32Array.of(10, 20), distance: Float32Array.of(900, 1200), visible: Uint8Array.of(1, 1) },
    credits: ['Terrain test data'],
};
const picture: MapPictureResult = { image: new Blob(['map']), points: { x: Float32Array.of(1, 2), y: Float32Array.of(1, 2), eye: Uint8Array.of(129, 129) }, credits: ['Map test data'] };
const failure = (name: string) => Object.assign(new Error(name), { name });

/** A fake TrailView: each still waits until the test settles it, or rejects as `still` does when its signal aborts. */
function fake() {
    const stills: { job: StillJob; resolve: (still: Still) => void; reject: (error: Error) => void }[] = [];
    const listeners = new Set<() => void>();
    let open = false;
    const renderer: Renderer = {
        canRender: async () => true,
        view: async (_, when) => ({ lat: 46.8, lon: 8.4, heading: 0, pitch: -30, mode: 'fly', height: 3000, debug: 'off', ...when }),
        still: (job, signal) => new Promise((resolve, reject) => {
            stills.push({ job, resolve, reject });
            signal.addEventListener('abort', () => reject(failure('aborted')));
        }),
        trailViewOpen: () => open,
        onTrailViewOpen: listener => { listeners.add(listener); return () => listeners.delete(listener); },
    };
    const setOpen = (value: boolean) => { open = value; listeners.forEach(listener => listener()); };
    return { renderer, stills, setOpen };
}

/** A fake map picture: each one waits until the test settles it. */
function fakePicture() {
    const calls: { resolve: (made: MapPictureResult | 'none') => void; reject: (error: Error) => void }[] = [];
    const maker: MapPictureMaker = (_, __, signal) => new Promise((resolve, reject) => {
        calls.push({ resolve, reject });
        signal.addEventListener('abort', () => reject(failure('aborted')));
    });
    return { maker, calls };
}

/** Lets the fake database and the job run, with no move of the fake clock. */
const settle = async () => { for (let i = 0; i < 30; i++) await new Promise(resolve => setImmediate(resolve)); };

let held = false;
const document = Object.assign(new EventTarget(), { hidden: false });
let library: Library, factory: IDBFactory, stops: (() => void)[];

beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    vi.spyOn(console, 'info').mockImplementation(() => {});
    held = false;
    document.hidden = false;
    vi.stubGlobal('document', document);
    vi.stubGlobal('window', new EventTarget());
    // One lock for all the jobs, as the browser has for all its tabs.
    vi.stubGlobal('navigator', {
        onLine: true,
        gpu: {},
        locks: {
            async request<T>(_: string, __: unknown, work: (lock: object | null) => Promise<T>) {
                if (held) return work(null);
                held = true;
                try { return await work({}); } finally { held = false; }
            },
        },
    });
    factory = new IDBFactory();
    library = new Library(factory);
    stops = [];
});

afterEach(() => {
    stops.forEach(stop => stop());
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

async function start(renderer: Renderer, on = library, picture = fakePicture().maker) {
    stops.push(startCoverJob(on, async () => renderer, async () => picture));
    await settle();
}

async function saved(...ids: string[]) {
    for (const id of ids) await library.saveRoute(route(id));
}

describe('settled', () => {
    it('is false for the image of an earlier line and for a waiting cover, so a page asks for them', () => {
        const item = route('a'), key = lineKey(item), image = new Blob(['cover']);
        expect(settled(item, undefined)).toBe(false);
        expect(settled(item, { id: 'a', lineKey: lineKey(route('a', 'a', [8, 46, 8.1, 46])), state: 'done', image, tries: 0 })).toBe(false);
        expect(settled(item, { id: 'a', lineKey: key, state: 'waiting', image, tries: 1 })).toBe(false);
        for (const state of ['done', 'none', 'failed'] as const) expect(settled(item, { id: 'a', lineKey: key, state, tries: 0 })).toBe(true);
    });
});

describe('the cover job', () => {
    it('renders one cover at a time in the whole browser, and skips a cover that another tab made', async () => {
        await saved('a');
        const first = fake(), second = fake();
        await start(first.renderer);
        wantCover('a', 'page');
        await settle();
        // The second tab asks for the same cover while the first renders it.
        await start(second.renderer, new Library(factory));
        wantCover('a', 'page');
        await settle();
        expect([first.stills.length, second.stills.length]).toEqual([1, 0]);
        first.stills[0].resolve(result);
        await settle();
        await vi.advanceTimersByTimeAsync(10_000);
        await settle();
        expect(second.stills).toHaveLength(0);
        const cover = await library.cover('a');
        expect(cover).toMatchObject({ state: 'done', lineKey: lineKey(route('a')), tries: 0, credits: ['Terrain test data'] });
        expect(cover?.points?.x).toEqual(Float32Array.of(10, 20));
        expect(first.stills[0].job).toMatchObject({ width: 1920, height: 1280, view: { date: '2026-08-15', time: '10:00', weather: 'sunny' } });
    });

    it('waits while the page is hidden', async () => {
        await saved('a');
        const { renderer, stills } = fake();
        document.hidden = true;
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        expect(stills).toHaveLength(0);
        document.hidden = false;
        document.dispatchEvent(new Event('visibilitychange'));
        await settle();
        expect(stills).toHaveLength(1);
    });

    it('stops a render when the planner opens TrailView, and renders it again when TrailView closes', async () => {
        await saved('a');
        const { renderer, stills, setOpen } = fake();
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        setOpen(true);
        await settle();
        expect(await library.cover('a')).toBeUndefined();
        setOpen(false);
        await settle();
        expect(stills).toHaveLength(2);
    });

    it.each([
        ['hidden', undefined, 2],
        ['tiles-failed', { state: 'waiting', tries: 1 }, 1],
        ['device-lost', { state: 'waiting', tries: 1 }, 2],
        ['timeout', { state: 'waiting', tries: 1 }, 1],
    ])('after %s, it counts %o and renders %i stills of the record', async (name, cover, renders) => {
        await saved('a');
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        // A hidden page waits; here it is visible again at once.
        stills[0].reject(failure(name));
        await settle();
        document.dispatchEvent(new Event('visibilitychange'));
        await settle();
        expect(await library.cover('a')).toEqual(cover && expect.objectContaining(cover));
        expect(stills).toHaveLength(renders);
    });

    it('goes on to the next record after tiles fail, and renders no cover after a time limit', async () => {
        await saved('a', 'b', 'c');
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('b', 'page');
        wantCover('a', 'page');
        await settle();
        stills[0].reject(failure('tiles-failed'));
        await settle();
        expect(stills.map(still => still.job.line)).toHaveLength(2);
        stills[1].reject(failure('timeout'));
        await settle();
        wantCover('c', 'page');
        await settle();
        expect(stills).toHaveLength(2);
    });

    it('marks a cover as failed after three tries, also of earlier sessions', async () => {
        await saved('a');
        await library.saveCover({ id: 'a', lineKey: lineKey(route('a')), state: 'waiting', tries: 2 });
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        stills[0].reject(failure('tiles-failed'));
        await settle();
        expect(await library.cover('a')).toMatchObject({ state: 'failed', tries: 3 });
        wantCover('a', 'page');
        await settle();
        expect(stills).toHaveLength(1);
    });

    it('renders the planner route 30 s after its line changes, and not after a new name', async () => {
        const { renderer, stills } = fake();
        await start(renderer);
        await library.saveRoute(route('a'));
        await library.setState('active', 'a');
        await settle();
        await vi.advanceTimersByTimeAsync(29_000);
        expect(stills).toHaveLength(0);
        await vi.advanceTimersByTimeAsync(1_000);
        await settle();
        expect(stills).toHaveLength(1);
        stills[0].resolve(result);
        await settle();
        await library.saveRoute(route('a', 'New name'));
        await settle();
        await vi.advanceTimersByTimeAsync(30_000);
        await settle();
        expect(stills).toHaveLength(1);
        await library.saveRoute(route('a', 'New name', [8.38, 46.82, 8.45, 46.8]));
        await settle();
        await vi.advanceTimersByTimeAsync(30_000);
        await settle();
        expect(stills).toHaveLength(2);
    });

    it('renders no cover for a saved route that is not the open plan, such as an imported one', async () => {
        const { renderer, stills } = fake();
        await start(renderer);
        await library.setState('active', 'a');
        await saved('a', 'b');
        await settle();
        await vi.advanceTimersByTimeAsync(30_000);
        await settle();
        expect(stills.map(still => still.job.line)).toHaveLength(1);
        stills[0].resolve(result);
        await settle();
        expect(await library.cover('b')).toBeUndefined();
    });

    it('renders a record again when it is asked for during its render, for the new line', async () => {
        await saved('a');
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        const line = [8.38, 46.82, 8.45, 46.8];
        await library.saveRoute(route('a', 'a', line));
        wantCover('a', 'page');
        stills[0].resolve(result);
        await settle();
        expect(stills).toHaveLength(2);
        expect(stills[1].job.line?.flat()).toEqual(line);
    });

    it('stores no cover for a route that is deleted during its render', async () => {
        await saved('a');
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('a', 'page');
        await settle();
        await library.removeRoute('a');
        stills[0].resolve(result);
        await settle();
        expect(await library.cover('a')).toBeUndefined();
    });

    it('keeps an ask that comes before the job starts', async () => {
        await saved('a');
        const { renderer, stills } = fake();
        wantCover('a', 'page');
        await start(renderer);
        expect(stills).toHaveLength(1);
    });
});

describe('the map picture', () => {
    /** A job with no WebGPU. */
    async function noGpu() {
        delete (navigator as { gpu?: object }).gpu;
        const renderer = vi.fn<() => Promise<Renderer>>(), { maker, calls } = fakePicture();
        stops.push(startCoverJob(library, renderer, async () => maker));
        await settle();
        return { renderer, calls };
    }

    it('comes with no WebGPU for a card and not for a page, and TrailView does not load', async () => {
        await saved('a', 'b');
        const { renderer, calls } = await noGpu();
        wantCover('b', 'page');
        wantCover('a', 'card');
        await settle();
        expect(calls).toHaveLength(1);
        calls[0].resolve(picture);
        await settle();
        expect(calls).toHaveLength(1);
        expect(renderer).not.toHaveBeenCalled();
        expect(await library.cover('a')).toMatchObject({
            state: 'waiting', tries: 0, map: { lineKey: lineKey(route('a')), tries: 0, light: expect.any(Blob), credits: ['Map test data'] },
        });
        expect(await library.cover('b')).toBeUndefined();
    });

    it('waits while a cover renders, and comes after a cover that is not possible', async () => {
        const { renderer, stills } = fake(), { maker, calls } = fakePicture(), view = renderer.view;
        renderer.view = async (line, when) => line[0][0] === 9 ? undefined : view(line, when);
        await library.saveRoute(route('a'));
        await library.saveRoute(route('b', 'b', [9, 47, 9.1, 47]));
        await start(renderer, library, maker);
        wantCover('a', 'card');
        await settle();
        expect([stills.length, calls.length]).toEqual([1, 0]);
        wantCover('b', 'card');
        stills[0].resolve(result);
        await settle();
        expect(calls).toHaveLength(1);
        calls[0].resolve(picture);
        await settle();
        expect(await library.cover('b')).toMatchObject({ state: 'none', map: { light: expect.any(Blob) } });
        expect((await library.cover('a'))?.map).toBeUndefined();
    });

    it.each([
        ['none', 1, { none: true }],
        ['tiles-failed', 3, { tries: 3 }],
    ])('stops for its line after %s, at %i pictures', async (end, count, map) => {
        await saved('a');
        const { calls } = await noGpu();
        for (let i = 0; i < 4; i++) {
            wantCover('a', 'card');
            await settle();
            if (end === 'none') calls[i]?.resolve('none');
            else calls[i]?.reject(failure(end));
            await settle();
        }
        expect(calls).toHaveLength(count);
        expect((await library.cover('a'))?.map).toMatchObject(map);
    });

    it('goes when the cover is done', async () => {
        await saved('a');
        const key = lineKey(route('a')), waiting: CoverRecord = { id: 'a', lineKey: key, state: 'waiting', tries: 1, map: { lineKey: key, tries: 0, light: new Blob(['map']) } };
        await library.saveCover(waiting);
        const { renderer, stills } = fake();
        await start(renderer);
        wantCover('a', 'card');
        await settle();
        stills[0].resolve(result);
        await settle();
        expect(await library.cover('a')).toMatchObject({ state: 'done' });
        expect((await library.cover('a'))?.map).toBeUndefined();
    });
});
