/** Renders the covers of routes and rides in the background, one at a time in the whole browser, and stores them in the
 * library, with a map picture for a card whose record gets no cover. */
import type { CoverRecord, MapPicture, RideRecord, RouteRecord } from '../library/records';
import type { Library } from '../library/store';
import { lineKey } from '../library/summary';
import { loadTrailView } from '../planner/trailview';
import { appearance } from '../theme.svelte';
import type { Still, StillJob } from '../trailview/still';
import { TIME_ZONE, type View } from '../trailview/view';
import { pack } from './line';
import type { MapPictureMaker } from './map-picture';

export const WIDTH = 1920, HEIGHT = 1280;
/** A changed route waits this long after its last change, so a rider who edits a route gets one cover at the end. */
const SETTLE_MS = 30_000;
/** A tab that finds the lock of another tab tries again after this. */
const RETRY_MS = 10_000;
const MAX_TRIES = 3;
const LOCK = 'obc-cover';

/** What the job uses of TrailView. A test gives fakes. */
export interface Renderer {
    /** False with no WebGPU or with a software renderer (`canRenderStills`). */
    canRender(): Promise<boolean>;
    /** The cover view of a line (`coverView`), or undefined when no view holds it or TrailView has no data there. */
    view(line: [number, number][], when: Pick<View, 'date' | 'time' | 'weather'>): Promise<View | undefined>;
    /** A still on the page's GPU device; it rejects with the named errors of `still`. */
    still(job: StillJob, signal: AbortSignal): Promise<Still>;
    /** Whether the planner shows TrailView, and a listener for each change. */
    trailViewOpen(): boolean;
    onTrailViewOpen(listener: () => void): () => void;
}

/** Changes when the covers of a line change, so that the job makes them again; map pictures keep theirs. */
const COVER_VERSION = 2;

/** The key that a cover keeps in its field `lineKey`: the line of the record and COVER_VERSION. */
export const coverKey = (item: RouteRecord | RideRecord) => `${lineKey(item)}/${COVER_VERSION}`;

/** Whether the job has nothing left to do for the record: its cover is made, impossible or failed for its line. A page
 * asks for the cover of a record that is not settled, also when it shows the cover of an earlier line. */
export const settled = (item: RouteRecord | RideRecord, cover: CoverRecord | undefined) => cover?.lineKey === coverKey(item) && cover.state !== 'waiting';

/** Where a cover part shows: a row of a list asks for nothing, a card for the cover and else for the map picture, a page
 * for the cover only. */
export type Place = 'row' | 'card' | 'page';
type Asker = Exclude<Place, 'row'>;

/** Whether the job can still make the map picture of the theme for the line of the record: it has no cover image, and
 * the picture of its line has no image of the theme, no `none` and fewer than MAX_TRIES tries. */
export function wantsPicture(item: RouteRecord | RideRecord, cover: CoverRecord | undefined, dark: boolean): boolean {
    const map = cover?.map?.lineKey === lineKey(item) ? cover.map : undefined;
    return !cover?.image && !map?.none && (map?.tries ?? 0) < MAX_TRIES && !map?.[dark ? 'dark' : 'light'];
}

let asked: ((id: string, place: Asker) => void) | undefined;
/** The asks that come before a job starts. */
const early: [string, Asker][] = [];

/** Asks for the cover of a route or a ride, if it has none for its line, and for a card also for the map picture. The
 * newest ask comes first. */
export function wantCover(id: string, place: Asker): void {
    if (asked) asked(id, place);
    else early.push([id, place]);
}

/**
 * Starts the cover job of the page. It renders the covers that `wantCover` asks for, and the cover of the planner's
 * route SETTLE_MS after its last change. For a card whose record gets no cover, it then makes the map picture of the
 * theme. It works only while the page is visible and online. A cover also waits while the planner shows TrailView; a
 * picture does not. Covers are off with no WebGPU or a coarse pointer and, once the job learns it, with a software renderer or after a still
 * that reached its time limit. Returns the function that stops the job.
 */
export function startCoverJob(library: Library, renderer: () => Promise<Renderer> = trailView, picture: () => Promise<MapPictureMaker> = mapPicture): () => void {
    if ((navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData) return () => {};
    const queue: string[] = [], cards = new Set<string>(), parked = new Set<string>(), settling = new Map<string, ReturnType<typeof setTimeout>>(), stops: (() => void)[] = [];
    // A phone gets no covers, as a browser with no WebGPU: its cards show the map picture.
    let engine: Promise<Renderer | undefined> | undefined, off = !('gpu' in navigator) || matchMedia('(pointer: coarse)').matches, busy = false, stopped = false;
    let rendering: AbortController | undefined, drawing: AbortController | undefined, retry: ReturnType<typeof setTimeout> | undefined;

    const stop = () => {
        stopped = true;
        rendering?.abort();
        drawing?.abort();
        clearTimeout(retry);
        for (const timer of settling.values()) clearTimeout(timer);
        for (const end of stops) end();
        if (asked === want) asked = undefined;
    };
    const want = (id: string, place: Asker) => {
        if (place === 'card') cards.add(id);
        if (queue.includes(id)) queue.splice(queue.indexOf(id), 1);
        queue.unshift(id);
        void pump();
    };
    asked = want;

    // The renderer loads at the first cover, so a page that asks for none does not load the engine.
    const load = () => engine ??= renderer().then(async loaded => {
        if (!await loaded.canRender()) return undefined;
        stops.push(loaded.onTrailViewOpen(() => {
            if (loaded.trailViewOpen()) return rendering?.abort();
            for (const id of parked) if (!queue.includes(id)) queue.push(id);
            parked.clear();
            void pump();
        }));
        return loaded;
    });
    const waits = () => stopped || document.hidden || !navigator.onLine;

    async function pump(): Promise<void> {
        if (busy || waits() || !queue.length) return;
        busy = true;
        try {
            while (queue.length && !waits()) {
                // An ask during the work puts the id in the queue again, for the line that it has then.
                const id = queue.shift()!;
                const outcome = await navigator.locks.request(LOCK, { ifAvailable: true }, lock => lock && work(id));
                if (outcome === 'park') parked.add(id);
                else if (outcome === 'next') { if (!queue.includes(id)) cards.delete(id); }
                else if (!queue.includes(id)) queue.unshift(id);
                if (!outcome) {
                    retry = setTimeout(() => void pump(), RETRY_MS);
                    return;
                }
            }
        } catch (error) {
            console.error('Cover job:', error);
            stop();
        } finally {
            busy = false;
        }
    }

    /** The work for one record, inside the lock. `stay` puts the record first in the queue again; `park` keeps it until
     * the planner closes TrailView. */
    async function work(id: string): Promise<'next' | 'stay' | 'park'> {
        const item = await library.item(id);
        let cover = await library.cover(id);
        // A record that is gone.
        if (!item?.summary) return 'next';
        if (!off && !settled(item, cover)) {
            const loaded = await load();
            if (!loaded) off = true;
            else if (loaded.trailViewOpen()) return 'park';
            else {
                const outcome = await render(loaded, id, item, cover);
                if (outcome !== 'next') return outcome;
                cover = await library.cover(id);
            }
        }
        const coverless = off || (settled(item, cover) && cover!.state !== 'done');
        return cards.has(id) && coverless && wantsPicture(item, cover, appearance.dark) ? draw(id, item, cover) : 'next';
    }

    /** One cover. */
    async function render(loaded: Renderer, id: string, item: RouteRecord | RideRecord, old: CoverRecord | undefined): Promise<'next' | 'stay'> {
        const key = coverKey(item), tries = old?.lineKey === key ? old.tries : 0, controller = rendering = new AbortController();
        const line = Array.from({ length: item.summary!.line.length / 2 }, (_, i) => [item.summary!.line[2 * i], item.summary!.line[2 * i + 1]] as [number, number]);
        try {
            const view = await loaded.view(line, when(item));
            if (!view) {
                await library.saveCover({ id, lineKey: key, state: 'none', tries: 0, ...old?.map && { map: old.map } });
                return 'next';
            }
            if (controller.signal.aborted) return 'stay';
            const { image, points, credits } = await loaded.still({ view, width: WIDTH, height: HEIGHT, line }, controller.signal);
            await library.saveCover({ id, lineKey: key, state: 'done', image, points: pack(points!), credits, tries: 0 });
            return 'next';
        } catch (error) {
            const name = error instanceof Error ? error.name : '';
            if (name === 'aborted' || name === 'hidden') return 'stay';
            console.info(`Cover of ${id}:`, error);
            // The cover of an earlier line and the map picture stay.
            await library.saveCover({ ...old, id, lineKey: key, state: tries + 1 >= MAX_TRIES ? 'failed' : 'waiting', tries: tries + 1 });
            if (name === 'timeout') off = true;
            // After a lost device, the next still asks for a new one.
            return name === 'device-lost' ? 'stay' : 'next';
        } finally {
            rendering = undefined;
        }
    }

    /** The map picture of the theme. */
    async function draw(id: string, item: RouteRecord | RideRecord, old: CoverRecord | undefined): Promise<'next' | 'stay'> {
        const key = lineKey(item), dark = appearance.dark, controller = drawing = new AbortController();
        const map: MapPicture = old?.map?.lineKey === key ? old.map : { lineKey: key, tries: 0 };
        // A record with a picture and no cover waits for a cover.
        const cover: CoverRecord = old ?? { id, lineKey: coverKey(item), state: 'waiting', tries: 0 };
        // The browser keeps a failed import until the page loads again, so it counts no try; the next ask tries again.
        const make = await picture().catch((error: unknown) => void console.info('Map picture code:', error));
        if (!make) return 'next';
        try {
            const made = await make(item.summary!, dark, controller.signal);
            const next: MapPicture = made === 'none' ? { lineKey: key, tries: 0, none: true }
                : { ...map, tries: 0, points: made.points, credits: made.credits, [dark ? 'dark' : 'light']: made.image };
            await library.saveCover({ ...cover, map: next });
            return 'next';
        } catch (error) {
            const name = error instanceof Error ? error.name : '';
            if (name === 'aborted' || name === 'hidden') return 'stay';
            console.info(`Map picture of ${id}:`, error);
            await library.saveCover({ ...cover, map: { ...map, tries: map.tries + 1 } });
            return 'next';
        } finally {
            drawing = undefined;
        }
    }

    stops.push(library.onChange((store, id) => {
        if (store !== 'routes') return;
        clearTimeout(settling.get(id));
        settling.set(id, setTimeout(async () => {
            settling.delete(id);
            // Only the planner's route: a restored backup changes many routes, and a row of a list asks for no cover.
            if (await library.state('active') !== id) return;
            const [item, cover] = await Promise.all([library.item(id), library.cover(id)]);
            if (item?.summary && !settled(item, cover)) want(id, 'page');
        }, SETTLE_MS));
    }));
    for (const [target, event] of [[document, 'visibilitychange'], [window, 'online']] as const) {
        const listener = () => void pump();
        target.addEventListener(event, listener);
        stops.push(() => target.removeEventListener(event, listener));
    }
    for (const [id, place] of early.splice(0)) want(id, place);
    return stop;
}

/** The time of a cover: the middle of a ride, or the day of the last change of a route at 10:00, sunny. */
function when(item: RouteRecord | RideRecord): Pick<View, 'date' | 'time' | 'weather'> {
    const ride = 'key' in item, at = ride ? new Date((item.summary.start + item.summary.seconds / 2) * 1000) : new Date(item.updatedAt);
    // Swedish writes a date and a time as YYYY-MM-DD HH:MM.
    const [date, time] = new Intl.DateTimeFormat('sv-SE', { timeZone: TIME_ZONE, dateStyle: 'short', timeStyle: 'short' }).format(at).split(' ');
    return { date, time: ride ? time : '10:00', weather: 'sunny' };
}

async function trailView(): Promise<Renderer> {
    const [start, { canRenderStills, still }, { coverView }, { config }] = await Promise.all([
        loadTrailView(), import('../trailview/still'), import('../trailview/still-view'), import('../planner/map-data'),
    ]);
    return {
        canRender: () => canRenderStills(),
        view: (line, conditions) => coverView(line, config, { width: WIDTH, height: HEIGHT, ...conditions }),
        async still(job, signal) {
            const gpu = await start.requestGpu();
            if (!gpu) throw new Error('No WebGPU device.');
            return still(gpu, config, job, { signal });
        },
        trailViewOpen: start.trailViewOpen,
        onTrailViewOpen: start.onTrailViewOpen,
    };
}

/** The map code loads at the first picture, so the shell does not load MapLibre at its start. */
const mapPicture = () => import('./map-picture').then(module => module.mapPicture);
