/** Starts the device sync of the shell, and shows the pages if it runs and what it did. */
import { untrack } from 'svelte';
import { deviceHolder } from '../device/session.svelte';
import type { Library } from '../library/store';
import { onRemember, remembered } from './remembered.svelte';
import type { SyncReport } from './sync';

/** One sync at a time in the whole browser: two tabs on one cable would send each route twice. */
const LOCK = 'obc-device-sync';

class DeviceSync {
    running = $state(false);
    problem = $state<string | null>(null);
    report = $state.raw<SyncReport | null>(null);
    private start = () => {};

    /** Starts a sync with the remembered device when it is ready, or after the sync that runs. */
    run(): void { this.start(); }

    /** For `startDeviceSync` only. */
    setStart(start: () => void): void { this.start = start; }
}

export const deviceSync = new DeviceSync();

/**
 * Syncs with the remembered device on each `ready` of it, after `rememberDevice`, on `deviceSync.run()`, and when a route
 * starts to wait or to leave. With no remembered device, it opens no device session and never syncs. Returns the function
 * that stops it.
 */
export function startDeviceSync(library: Library): () => void {
    let again = false;
    /** The ids that the last sync tried and left in `waiting` or `leaving`, such as a route that the routing could not
     * calculate offline: only a new id, or the next connection, tries them again. */
    let tried = new Set<string>();

    async function pending(): Promise<boolean> {
        const ids = new Set([...await library.state('waiting') ?? [], ...await library.state('leaving') ?? []]);
        tried = new Set([...tried].filter(id => ids.has(id)));
        return [...ids].some(id => !tried.has(id));
    }

    async function run(): Promise<void> {
        const session = deviceHolder.session, client = session?.client, serial = session?.info?.serialNumber;
        if (session?.status !== 'ready' || !client || !serial || remembered.device?.serial !== serial) return;
        if (deviceSync.running) { again = true; return; }
        deviceSync.running = true;
        try {
            const { syncDevice } = await import('./sync');
            const work = () => syncDevice(client, serial, library, new AbortController().signal);
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
        }
        if (again) {
            again = false;
            if (await pending()) void run();
        }
    }

    deviceSync.setStart(() => void run());
    if (remembered.device) void deviceHolder.open();
    const stopEffect = $effect.root(() => {
        $effect(() => { if (deviceHolder.session?.status === 'ready') untrack(() => void run()); });
    });
    const stopRemember = onRemember(() => { tried.clear(); void run(); });
    const stopChange = library.onChange((store, key) => {
        if (store !== 'state' || (key !== 'waiting' && key !== 'leaving')) return;
        if (deviceSync.running) again = true;
        else void pending().then(next => { if (next) void run(); });
    });
    return () => { stopEffect(); stopRemember(); stopChange(); deviceSync.setStart(() => {}); };
}
