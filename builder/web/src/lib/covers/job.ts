/** Renders the covers of routes and rides in the background, one at a time in the whole browser, and stores them in the library. */
import type { CoverRecord, RideRecord, RouteRecord } from '../library/records';
import type { Library } from '../library/store';
import { loadTrailView } from '../planner/trailview';
import type { Still, StillJob } from '../trailview/still';
import { TIME_ZONE, type View } from '../trailview/view';
import { pack } from './line';

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

/** A hash of the summary line, and for a ride of its start time, as hex. A new name keeps it. */
export function lineKey(item: RouteRecord | RideRecord): string {
    const text = `${item.summary?.line.join(',')}@${'key' in item ? item.summary.start : ''}`;
    // FNV-1a
    let hash = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 0x01000193);
    return (hash >>> 0).toString(16).padStart(8, '0');
}

/** Whether the job has nothing left to do for the record: its cover is made, impossible or failed for its line. A page
 * asks for the cover of a record that is not settled, also when it shows the cover of an earlier line. */
export const settled = (item: RouteRecord | RideRecord, cover: CoverRecord | undefined) => cover?.lineKey === lineKey(item) && cover.state !== 'waiting';

let asked: ((id: string) => void) | undefined;
/** The asks that come before a job starts. */
const early: string[] = [];

/** Asks for the cover of a route or a ride, if it has none for its line. The newest ask renders first. */
export function wantCover(id: string): void {
    if (asked) asked(id);
    else if (!early.includes(id)) early.push(id);
}

/**
 * Starts the cover job of the page. It renders the covers that `wantCover` asks for, and the cover of the planner's
 * route SETTLE_MS after its last change. It renders only while the page is visible and online and while the planner
 * shows no TrailView, and stops for the session after a still that reached its time limit. Returns the function that
 * stops it.
 */
export function startCoverJob(library: Library, renderer: () => Promise<Renderer> = trailView): () => void {
    if ((navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData) return () => {};
    const queue: string[] = [], settling = new Map<string, ReturnType<typeof setTimeout>>(), stops: (() => void)[] = [];
    let engine: Promise<Renderer | undefined> | undefined, busy = false, stopped = false, rendering: AbortController | undefined, retry: ReturnType<typeof setTimeout> | undefined;

    const stop = () => {
        stopped = true;
        rendering?.abort();
        clearTimeout(retry);
        for (const timer of settling.values()) clearTimeout(timer);
        for (const end of stops) end();
        if (asked === want) asked = undefined;
    };
    const want = (id: string) => {
        if (queue.includes(id)) queue.splice(queue.indexOf(id), 1);
        queue.unshift(id);
        void pump();
    };
    asked = want;

    // The renderer loads at the first cover, so a page that asks for none does not load the engine.
    const load = () => engine ??= renderer().then(async loaded => {
        if (!await loaded.canRender()) return undefined;
        stops.push(loaded.onTrailViewOpen(() => loaded.trailViewOpen() ? rendering?.abort() : void pump()));
        return loaded;
    });
    const waits = (loaded?: Renderer) => stopped || document.hidden || !navigator.onLine || !!loaded?.trailViewOpen();

    async function pump(): Promise<void> {
        if (busy || waits() || !queue.length) return;
        busy = true;
        try {
            const loaded = await load();
            if (!loaded) return stop();
            while (queue.length && !waits(loaded)) {
                // An ask during the render puts the id in the queue again, for the line that it has then.
                const id = queue.shift()!;
                const outcome = await navigator.locks.request(LOCK, { ifAvailable: true }, lock => lock && render(loaded, id));
                if ((!outcome || outcome === 'stay') && !queue.includes(id)) queue.unshift(id);
                if (!outcome) {
                    retry = setTimeout(() => void pump(), RETRY_MS);
                    return;
                }
                if (outcome === 'stop') return stop();
            }
        } catch (error) {
            console.error('Cover job:', error);
            stop();
        } finally {
            busy = false;
        }
    }

    /** One cover, inside the lock. `stay` puts the record first in the queue again. */
    async function render(loaded: Renderer, id: string): Promise<'next' | 'stay' | 'stop'> {
        const [item, old] = await Promise.all([library.item(id), library.cover(id)]);
        // The cover of another tab, or a record that is gone.
        if (!item?.summary || settled(item, old)) return 'next';
        const key = lineKey(item), tries = old?.lineKey === key ? old.tries : 0, controller = rendering = new AbortController();
        const line = Array.from({ length: item.summary.line.length / 2 }, (_, i) => [item.summary!.line[2 * i], item.summary!.line[2 * i + 1]] as [number, number]);
        try {
            const view = await loaded.view(line, when(item));
            if (!view) {
                await library.saveCover({ id, lineKey: key, state: 'none', tries: 0 });
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
            // The cover of an earlier line stays.
            await library.saveCover({ ...old, id, lineKey: key, state: tries + 1 >= MAX_TRIES ? 'failed' : 'waiting', tries: tries + 1 });
            // After a lost device, the next still asks for a new one.
            return name === 'timeout' ? 'stop' : name === 'device-lost' ? 'stay' : 'next';
        } finally {
            rendering = undefined;
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
            if (item?.summary && !settled(item, cover)) want(id);
        }, SETTLE_MS));
    }));
    for (const [target, event] of [[document, 'visibilitychange'], [window, 'online']] as const) {
        const listener = () => void pump();
        target.addEventListener(event, listener);
        stops.push(() => target.removeEventListener(event, listener));
    }
    for (const id of early.splice(0)) want(id);
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
