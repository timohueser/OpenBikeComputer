// @vitest-environment happy-dom

import { flushSync } from 'svelte';
import { describe, expect, it } from 'vitest';
import type { MapRun } from '../coverage/run.svelte';
import type { CoverageStore } from '../coverage/store.svelte';
import { ready } from './map.svelte';

/** The parts of a store and a run that `ready` reads, as state that the test changes. */
function stubs() {
    const store = $state({ indexError: null, resolutionError: null, regionErrors: new Map<string, string>(), ledger: { isFinal: false, cellCount: 0 } });
    const run = $state({ ready: false, refusal: null as string | null, estimateError: null as string | null });
    return { store, run, typed: [store as unknown as CoverageStore, run as unknown as MapRun] as const };
}

describe('ready', () => {
    it('lets the send start only after the cell lists and the memory check of the run are complete', async () => {
        const { store, run, typed } = stubs();
        let done = false;
        const waiting = ready(...typed, new AbortController().signal).then(() => { done = true; });
        flushSync();
        store.ledger = { isFinal: true, cellCount: 5 };
        flushSync();
        await Promise.resolve();
        expect(done, 'the memory check of the run has not answered').toBe(false);

        run.ready = true;
        flushSync();
        await waiting;
        expect(done).toBe(true);
    });

    it('ends with the refusal of the memory check, and never starts the send', async () => {
        const { store, run, typed } = stubs();
        const waiting = ready(...typed, new AbortController().signal);
        store.ledger = { isFinal: true, cellCount: 5 };
        run.refusal = 'Reduce the map selection.';
        flushSync();
        await expect(waiting).rejects.toThrow('Reduce the map selection.');
    });
});
