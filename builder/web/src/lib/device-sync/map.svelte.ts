/** Builds the device map that waits, in this tab, and sends it to the device. It needs the shared wasm core. */
import { CatalogClient } from '../catalog/client';
import { mapWorkBusy } from '../cells/workBarrier';
import { MapRun } from '../coverage/run.svelte';
import { CoverageStore } from '../coverage/store.svelte';
import type { JobContext } from '../device/progress';
import type { NextMap } from '../library/records';
import { platform } from '../platform';
import type { FlatStoreClient } from '../usb/client';
import type { PutResponse } from '../usb/protocol';
import { areaOf, sameArea, type MapArea } from './area';
import { until } from './until';

/**
 * Downloads the cells of `next`, assembles them and puts the map on the device as one run that `ctx.signal` stops.
 * It builds the area and the two map styles of `next` or nothing, with the colours that the styles have now, and gives
 * the area with the versions of the styles that it sent: the caller records it as the map on the device. `waits` is called when another
 * map build holds the working files. While the run goes, the browser asks before it closes the tab.
 */
export async function buildMap(next: NextMap, client: FlatStoreClient, ctx: JobContext, waits: () => void): Promise<PutResponse & { area: MapArea }> {
    const leave = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', leave);
    let run: MapRun | undefined;
    try {
        const { url, body } = await until(platform.catalog(), ctx.signal);
        const store = new CoverageStore(CatalogClient.fromBody(body, url, { fetchImpl: platform.catalogFetch }), body);
        store.setArea(next);
        // The selection leaves out a region that the catalog does not hold and a box that the grid refuses, and it takes
        // the first map style for one that is not there. A style with new colours since the rider chose the area is sent.
        const area = areaOf(store);
        if (!sameArea(area, next) || area.light !== next.light || area.dark !== next.dark) throw new Error('This area cannot be built: a part or a map style of it is not there any more. Choose the area again.');
        if (await until(mapWorkBusy(), ctx.signal)) waits();
        run = new MapRun(store);
        run.start();
        await ready(store, run, ctx.signal);
        return { ...await run.sendToDevice(client, ctx), area };
    } finally {
        window.removeEventListener('beforeunload', leave);
        run?.destroy();
    }
}

/** Resolves when the run can start, and rejects with the reason why it never can. */
export function ready(store: CoverageStore, run: MapRun, signal: AbortSignal): Promise<void> {
    return new Promise((resolve, reject) => {
        let stop = () => {};
        const end = (error?: unknown) => {
            signal.removeEventListener('abort', aborted);
            // The effect that calls this is inside the root that `stop` ends.
            queueMicrotask(() => stop());
            if (error === undefined) resolve();
            else reject(error);
        };
        const aborted = () => end(signal.reason);
        if (signal.aborted) return reject(signal.reason);
        signal.addEventListener('abort', aborted, { once: true });
        stop = $effect.root(() => {
            $effect(() => {
                const area = store.indexError ?? store.resolutionError ?? [...store.regionErrors.values()][0];
                if (area) end(new Error(`This area cannot be built: ${area}`));
                else if (store.ledger?.isFinal && store.ledger.cellCount === 0) end(new Error('This area cannot be built: the map catalog has no map data for it.'));
                else if (run.refusal) end(new Error(run.refusal));
                else if (run.estimateError) end(new Error(`The memory check failed: ${run.estimateError}`));
                else if (run.ready) end();
            });
        });
    });
}
