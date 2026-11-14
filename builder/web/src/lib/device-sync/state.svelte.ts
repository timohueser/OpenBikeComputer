/** Starts the device sync of the shell, and shows the pages if it runs and what it did. It also runs the send of the map
 * that waits, which only the rider starts. */
import { untrack } from 'svelte';
import { initCore } from '../core/bridge';
import type { JobContext } from '../device/progress';
import { deviceHolder } from '../device/session.svelte';
import type { Library } from '../library/store';
import { onRemember, remembered } from './remembered.svelte';
import type { SyncReport } from './sync';

/** One sync at a time in the whole browser: two tabs on one cable would send each route twice. */
const LOCK = 'obc-device-sync';

/** A step of the map send. `wait`: another map build holds the working files. `percent` is of the step. */
export interface MapStep { phase: 'wait' | 'download' | 'build' | 'send'; percent: number }

class DeviceSync {
    running = $state(false);
    problem = $state<string | null>(null);
    report = $state.raw<SyncReport | null>(null);
    /** The map send that runs, or null. */
    map = $state.raw<MapStep | null>(null);
    /** Why the last map send did not put the map on the device; `stopped` when the rider stopped it. */
    mapProblem = $state.raw<{ text: string; stopped: boolean } | null>(null);
    private start = () => {};
    private startMap = () => {};
    private stopMap = () => {};

    /** Starts a sync with the remembered device when it is ready, or after the sync that runs. */
    run(): void { this.start(); }

    /** Builds the map that waits and sends it to the remembered device, now or after the sync that runs. */
    sendMap(): void { this.startMap(); }

    /** Stops the map send that runs. */
    stop(): void { this.stopMap(); }

    /** For `startDeviceSync` only. */
    setStart(start: () => void, startMap = () => {}, stopMap = () => {}): void {
        this.start = start;
        this.startMap = startMap;
        this.stopMap = stopMap;
    }
}

export const deviceSync = new DeviceSync();

/**
 * Syncs with the remembered device on each `ready` of it, after `rememberDevice`, on `deviceSync.run()`, and when a route
 * starts to wait or to leave. With no remembered device, it opens no device session and never syncs. A sync never builds a
 * map: only `deviceSync.sendMap()` does. The sync and the map send use one link, so each waits for the other. Returns the
 * function that stops it.
 */
export function startDeviceSync(library: Library): () => void {
    /** A change of `waiting` or `leaving` came during a sync. */
    let again = false;
    /** A connection or a new remembered device came during a sync: the next sync runs with no check. */
    let fresh = false;
    /** Aborts the sync that runs when the device leaves `ready`. */
    let running: AbortController | null = null;
    /** The ids that the last sync tried and left in `waiting` or `leaving`, such as a route that the routing could not
     * calculate offline: only a new id, or the next connection, tries them again. */
    let tried = new Set<string>();
    /** Aborts the map send that runs. */
    let sending: AbortController | null = null;
    /** The rider stopped the map send that runs. */
    let stopped = false;
    /** The rider asked for the map during a sync: the send starts after it. */
    let mapWaits = false;

    async function pending(): Promise<boolean> {
        const ids = new Set([...await library.state('waiting') ?? [], ...await library.state('leaving') ?? []]);
        tried = new Set([...tried].filter(id => ids.has(id)));
        return [...ids].some(id => !tried.has(id));
    }

    async function run(): Promise<void> {
        const session = deviceHolder.session, client = session?.client, serial = session?.info?.serialNumber;
        // A map send ends with a sync.
        if (session?.status !== 'ready' || !client || !serial || remembered.device?.serial !== serial || deviceSync.map) return;
        if (deviceSync.running) { again = true; return; }
        deviceSync.running = true;
        const controller = running = new AbortController();
        try {
            const { syncDevice } = await import('./sync');
            const work = () => syncDevice(client, serial, library, controller.signal);
            const report = navigator.locks ? await navigator.locks.request(LOCK, { ifAvailable: true }, lock => lock ? work() : null) : await work();
            if (report) {
                deviceSync.report = report;
                deviceSync.problem = report.problem;
                tried = new Set(report.tried);
            }
        } catch (error) {
            deviceSync.problem = error instanceof Error ? error.message : String(error);
        } finally {
            deviceSync.running = false;
            running = null;
        }
        if (mapWaits) {
            mapWaits = fresh = again = false;
            void sendMap();
        } else if (fresh) {
            fresh = again = false;
            void run();
        } else if (again) {
            again = false;
            if (await pending()) void run();
        }
    }

    async function sendMap(): Promise<void> {
        const session = deviceHolder.session, client = session?.client, serial = session?.info?.serialNumber;
        if (session?.status !== 'ready' || !client || !serial || remembered.device?.serial !== serial || deviceSync.map) return;
        if (deviceSync.running) { mapWaits = true; return; }
        const controller = sending = new AbortController();
        stopped = false;
        deviceSync.mapProblem = null;
        let phase: MapStep['phase'] = 'download';
        const show = (percent = 0) => {
            if (deviceSync.map?.phase !== phase || deviceSync.map.percent !== percent) deviceSync.map = { phase, percent };
        };
        show();
        const ctx: JobContext = {
            signal: controller.signal,
            cancel: reason => controller.abort(reason),
            // After the last byte the device stores the map: the send stays at its end.
            phase: job => {
                phase = job === 'downloading' ? 'download' : ['sending', 'committing', 'finalizing'].includes(job) ? 'send' : 'build';
                show(phase === 'send' && job !== 'sending' ? 100 : 0);
            },
            progress: (done, total) => show(total ? Math.floor(100 * done / total) : 0),
        };
        try {
            const [sync, { buildMap }] = await Promise.all([import('./sync'), initCore().then(() => import('./map.svelte'))]);
            const waits = () => { phase = 'wait'; show(); };
            const problem = await sync.sendMap(serial, library, (next, job) => buildMap(next, client, job, waits), ctx);
            if (problem) deviceSync.mapProblem = { text: problem, stopped };
        } catch (error) {
            deviceSync.mapProblem = { text: error instanceof Error ? error.message : String(error), stopped: false };
        } finally {
            deviceSync.map = null;
            sending = null;
        }
        // The sync reads the card again, and sends the routes that waited.
        fresh = again = false;
        void run();
    }

    function stopMap(): void {
        stopped = true;
        sending?.abort(new DOMException('The rider stopped the map send.', 'AbortError'));
    }

    /** A sync now, or one more after the sync that runs, with no check of `tried`. */
    function connected(): void {
        if (deviceSync.running) fresh = true;
        else void run();
    }

    deviceSync.setStart(connected, () => void sendMap(), stopMap);
    if (remembered.device) void deviceHolder.open();
    const stopEffect = $effect.root(() => {
        $effect(() => {
            const session = deviceHolder.session, ready = session?.status === 'ready' && session.client;
            untrack(() => {
                if (ready) return connected();
                running?.abort();
                sending?.abort();
            });
        });
    });
    const stopRemember = onRemember(connected);
    const stopChange = library.onChange((store, key) => {
        if (store !== 'state' || (key !== 'waiting' && key !== 'leaving')) return;
        if (deviceSync.running) again = true;
        else void pending().then(next => { if (next) void run(); });
    });
    return () => { stopEffect(); stopRemember(); stopChange(); deviceSync.setStart(() => {}); };
}
